//! SQLite sessions built on `rusqlite` with a bundled SQLite.
//!
//! rusqlite is blocking, so all database work runs on tokio's blocking pool.
//! Result pages travel back to the async caller through a small bounded
//! channel: `emit` only ever runs on the caller's task, and the backpressure
//! keeps a fast statement from buffering its whole result in memory.
//!
//! A read that pauses at its fetch limit keeps its worker: the worker holds
//! the connection and the half-stepped statement and waits for the next
//! fetch request. While it waits, the statement's read transaction keeps a
//! shared lock on the file, so the app closes idle results after a while.

mod apply;
mod introspect;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectionParams, Engine, Error, Fetch, NO_OPEN_RESULT, Paged, Pager,
    QueryEvent, Result, Row, RowChange, SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, TableRef, Value,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, InterruptHandle, Rows};
use tokio::sync::{mpsc, oneshot};

/// How many VM instructions run between checks of the cancel flag.
const PROGRESS_INTERVAL: i32 = 1000;

/// Pages buffered between the blocking worker and the caller.
const PAGES_IN_FLIGHT: usize = 2;

pub struct SqliteSession {
    conn: Arc<Mutex<Connection>>,
    canceller: SqliteCanceller,
    info: ServerInfo,
    /// Whether a transaction was open after the last statement.
    in_transaction: bool,
    /// A read paused at its fetch limit.
    open: Option<OpenResult>,
}

/// A paused read: its worker, waiting for requests. Dropping `requests`
/// ends the worker, which releases the statement and the connection.
struct OpenResult {
    requests: std::sync::mpsc::Sender<FetchRequest>,
    worker: tokio::task::JoinHandle<()>,
}

/// Read on: pages go to `events`, the outcome to `done`.
struct FetchRequest {
    fetch: Fetch,
    events: mpsc::Sender<QueryEvent>,
    done: oneshot::Sender<Pumped>,
}

/// What a statement or a fetch did.
struct Pumped {
    /// Rows delivered, or changed for statements without a result set.
    rows: u64,
    /// Whether a cancel stopped it, or how it failed.
    result: rusqlite::Result<bool>,
    has_more: bool,
    in_transaction: bool,
}

/// Cancels the statement currently running on a session.
///
/// Cancellation is only ever delivered to a statement that is running:
/// `cancel()` on an idle session, or late duplicates arriving after the
/// statement ended, do nothing, so they cannot abort the next statement.
#[derive(Clone)]
pub struct SqliteCanceller {
    state: Arc<CancelState>,
}

struct CancelState {
    interrupt: InterruptHandle,
    /// Whether a statement is running. Guards `interrupt` so it never fires
    /// between statements.
    running: Mutex<bool>,
    /// Read by the progress handler and between pages.
    requested: Arc<AtomicBool>,
}

impl CancelState {
    fn begin(&self) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        *running = true;
        self.requested.store(false, Ordering::SeqCst);
    }

    fn finish(&self) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        *running = false;
        // Otherwise the progress handler would abort introspection queries
        // after a cancelled statement.
        self.requested.store(false, Ordering::SeqCst);
    }
}

impl Canceller for SqliteCanceller {
    async fn cancel(&self) -> Result<()> {
        let running = self.state.running.lock().unwrap_or_else(PoisonError::into_inner);
        // Only the first request interrupts; repeats are no-ops.
        if *running && !self.state.requested.swap(true, Ordering::SeqCst) {
            // `interrupt()` stops a statement mid-step. SQLite clears its
            // interrupt flag when a statement starts, so a request landing
            // between prepare and the first step is caught by the progress
            // handler instead, which polls `requested`.
            self.state.interrupt.interrupt();
        }
        Ok(())
    }
}

impl SqliteSession {
    /// Opens `params.path`, creating the file if it does not exist. The
    /// password is ignored: SQLite files have none.
    pub async fn connect(params: &ConnectionParams, _password: Option<&str>) -> Result<Self> {
        let path = params.path.trim().to_owned();
        if path.is_empty() {
            return Err(Error::InvalidParams("SQLite needs a database file path".into()));
        }

        let opened = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
            let conn = Connection::open(&path)?;
            conn.busy_timeout(Duration::from_secs(5))?;
            conn.execute_batch("PRAGMA foreign_keys = ON")?;
            // Touch the file so a missing directory or a non-database file
            // fails here rather than on the first statement.
            conn.query_row("select count(*) from main.sqlite_schema", [], |_| Ok(()))?;
            let version: String = conn.query_row("select sqlite_version()", [], |r| r.get(0))?;
            Ok((conn, version))
        })
        .await
        .map_err(|e| Error::Connect(e.to_string()))?;
        let (conn, version) = opened.map_err(|e| Error::Connect(e.to_string()))?;

        let requested = Arc::new(AtomicBool::new(false));
        let flag = requested.clone();
        conn.progress_handler(PROGRESS_INTERVAL, Some(move || flag.load(Ordering::SeqCst)))
            .map_err(|e| Error::Connect(e.to_string()))?;

        let canceller = SqliteCanceller {
            state: Arc::new(CancelState {
                interrupt: conn.get_interrupt_handle(),
                running: Mutex::new(false),
                requested,
            }),
        };
        let info = ServerInfo { engine: Engine::Sqlite, version, default_schema: Some("main".into()) };
        Ok(Self { conn: Arc::new(Mutex::new(conn)), canceller, info, in_transaction: false, open: None })
    }

    /// Hands a request to a worker and relays its pages to `emit`. `None`
    /// when the worker is gone.
    async fn relay(
        &self,
        send: impl FnOnce(FetchRequest) -> bool,
        fetch: Fetch,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Option<Pumped> {
        let (events, mut pages) = mpsc::channel(PAGES_IN_FLIGHT);
        let (done, outcome) = oneshot::channel();
        if !send(FetchRequest { fetch, events, done }) {
            return None;
        }
        while let Some(event) = pages.recv().await {
            emit(event);
        }
        outcome.await.ok()
    }

    /// Releases the paused read, if any (see [`Session::close_result`]).
    async fn close_open(&mut self) {
        if let Some(open) = self.open.take() {
            drop(open.requests);
            let _ = open.worker.await;
        }
    }

    /// Runs `work` against the connection on the blocking pool.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || work(&conn.lock().unwrap_or_else(PoisonError::into_inner)))
            .await
            .map_err(|e| Error::Query(format!("sqlite worker failed: {e}")))?
            .map_err(|e| Error::Query(e.to_string()))
    }
}

impl Session for SqliteSession {
    type Canceller = SqliteCanceller;

    fn canceller(&self) -> SqliteCanceller {
        self.canceller.clone()
    }

    fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    async fn execute(&mut self, sql: &str, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        self.close_open().await;
        let started = Instant::now();
        let state = self.canceller.state.clone();
        state.begin();

        let (requests, waiting) = std::sync::mpsc::channel();
        let conn = self.conn.clone();
        let sql = sql.to_owned();
        let requested = state.requested.clone();
        let fetch = Fetch { page_size: fetch.page_size.max(1), ..fetch };
        let mut worker = None;
        let pumped = self
            .relay(
                |first| {
                    worker = Some(tokio::task::spawn_blocking(move || {
                        let conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
                        serve(&conn, &sql, &requested, first, &waiting);
                    }));
                    true
                },
                fetch,
                emit,
            )
            .await;
        state.finish();
        let worker = worker.expect("the worker was spawned");
        self.settle(pumped, OpenResult { requests, worker }, started, emit).await;
    }

    async fn fetch_more(&mut self, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        let Some(open) = self.open.take() else {
            emit(QueryEvent::Error { message: NO_OPEN_RESULT.into(), position: None, in_transaction: self.in_transaction });
            return;
        };
        let started = Instant::now();
        let state = self.canceller.state.clone();
        state.begin();
        let fetch = Fetch { page_size: fetch.page_size.max(1), ..fetch };
        let pumped = self.relay(|request| open.requests.send(request).is_ok(), fetch, emit).await;
        state.finish();
        self.settle(pumped, open, started, emit).await;
    }

    async fn close_result(&mut self) {
        self.close_open().await;
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        self.close_open().await;
        self.blocking(introspect::schemas).await
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        self.close_open().await;
        let schema = schema.to_owned();
        self.blocking(move |conn| introspect::schema_model(conn, &schema)).await
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        self.close_open().await;
        let conn = self.conn.clone();
        let (table, changes) = (table.clone(), changes.to_vec());
        tokio::task::spawn_blocking(move || {
            apply::apply(&conn.lock().unwrap_or_else(PoisonError::into_inner), &table, &changes)
        })
        .await
        .map_err(|e| Error::Query(format!("sqlite worker failed: {e}")))?
        .map_err(|e| Error::Query(e.to_string()))
    }

    /// Compiling a statement resolves its tables and columns; only stepping
    /// it runs anything. `schema` does not apply: unqualified names resolve
    /// across the attached databases.
    async fn check(&mut self, sql: &str, _schema: Option<&str>) -> Result<Option<SqlProblem>> {
        self.close_open().await;
        let sql = sql.to_owned();
        self.blocking(move |conn| {
            Ok(match conn.prepare(&sql) {
                Ok(_) => None,
                // Not the user's error: the console sends statements one at a time.
                Err(rusqlite::Error::MultipleStatement) => None,
                Err(e) => Some(problem(&e)),
            })
        })
        .await
    }

    async fn set_schema(&mut self, _schema: &str) -> Result<()> {
        self.close_open().await;
        Ok(())
    }
}

impl SqliteSession {
    /// Reports how a statement or a fetch ended, keeping the worker when its
    /// read paused and ending it otherwise.
    async fn settle(
        &mut self,
        pumped: Option<Pumped>,
        open: OpenResult,
        started: Instant,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) {
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let Some(pumped) = pumped else {
            drop(open.requests);
            let failure = open.worker.await.err().map(|e| e.to_string()).unwrap_or_default();
            emit(QueryEvent::Error {
                message: format!("sqlite worker failed: {failure}"),
                position: None,
                in_transaction: self.in_transaction,
            });
            return;
        };
        self.in_transaction = pumped.in_transaction;
        if pumped.has_more {
            self.open = Some(open);
        } else {
            drop(open.requests);
            let _ = open.worker.await;
        }
        let Pumped { rows: row_count, result, has_more, in_transaction } = pumped;
        emit(match result {
            Ok(cancelled) => QueryEvent::Done { row_count, elapsed_ms, cancelled, has_more, in_transaction },
            Err(e) if e.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) => {
                QueryEvent::Done { row_count, elapsed_ms, cancelled: true, has_more: false, in_transaction }
            }
            Err(e) => error_event(&e, in_transaction),
        });
    }
}

fn error_event(e: &rusqlite::Error, in_transaction: bool) -> QueryEvent {
    let SqlProblem { message, position } = problem(e);
    QueryEvent::Error { message, position, in_transaction }
}

/// SQLite locates input errors with a byte offset into the statement; report
/// it as a char offset, and keep the message free of the echoed SQL that
/// rusqlite's `Display` appends.
fn problem(e: &rusqlite::Error) -> SqlProblem {
    match e {
        rusqlite::Error::SqlInputError { msg, sql, offset, .. } => SqlProblem {
            message: msg.clone(),
            position: usize::try_from(*offset)
                .ok()
                .and_then(|offset| sql.get(..offset))
                .map(|prefix| prefix.chars().count() as u32),
        },
        e => SqlProblem { message: e.to_string(), position: None },
    }
}

/// The worker of one statement, on the blocking pool with the connection
/// locked: runs it for `first`, then, while its read is paused at a fetch
/// limit, serves further fetch requests. Returns (releasing the statement
/// and the connection) when the rows run out or the session stops asking.
fn serve(
    conn: &Connection,
    sql: &str,
    requested: &AtomicBool,
    first: FetchRequest,
    requests: &std::sync::mpsc::Receiver<FetchRequest>,
) {
    // SQLite knows exactly whether a transaction is open: it is in
    // autocommit mode otherwise.
    let outcome = |rows, result, has_more| Pumped { rows, result, has_more, in_transaction: !conn.is_autocommit() };
    let FetchRequest { fetch, events, done } = first;
    let mut statement = match conn.prepare(sql) {
        Ok(statement) => statement,
        Err(e) => {
            let _ = done.send(outcome(0, Err(e), false));
            return;
        }
    };

    if statement.column_count() == 0 {
        // `changes()` keeps the count of the last INSERT/UPDATE/DELETE, so a
        // DDL statement would report a stale number. `total_changes()` only
        // moves when this statement changed rows.
        let before = conn.total_changes();
        let result = statement.execute([]).map(|_| false);
        let changed = if result.is_ok() && conn.total_changes() != before { conn.changes() } else { 0 };
        drop(events);
        let _ = done.send(outcome(changed, result, false));
        return;
    }

    let columns: Vec<Column> = statement
        .columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_owned(),
            type_name: c.decl_type().unwrap_or_default().to_lowercase(),
        })
        .collect();
    let width = columns.len();
    if events.blocking_send(QueryEvent::Columns { columns }).is_err() {
        return;
    }
    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(e) => {
            drop(events);
            let _ = done.send(outcome(0, Err(e), false));
            return;
        }
    };

    let mut held = None;
    let mut next = Some(FetchRequest { fetch, events, done });
    while let Some(FetchRequest { fetch, events, done }) = next.take() {
        let (delivered, result, has_more) = pump(&mut rows, width, fetch, requested, &events, &mut held);
        // Closing the pages channel ends the caller's relay before the outcome.
        drop(events);
        let _ = done.send(outcome(delivered, result, has_more));
        if has_more {
            next = requests.recv().ok();
        }
    }
}

/// Steps the statement up to the fetch limit, sending full pages. Returns
/// rows delivered, whether a cancel stopped it (or how it failed), and
/// whether it paused with rows left. A cancel ends the read: SQLite keeps
/// interrupting a statement once asked to.
fn pump(
    rows: &mut Rows<'_>,
    width: usize,
    fetch: Fetch,
    requested: &AtomicBool,
    events: &mpsc::Sender<QueryEvent>,
    held: &mut Option<Row>,
) -> (u64, rusqlite::Result<bool>, bool) {
    let mut pager = Pager::new(fetch);
    let mut delivered = 0u64;
    // False once the caller stopped listening (the session went away).
    let mut send = |page: Vec<Row>| {
        if page.is_empty() {
            return true;
        }
        delivered += page.len() as u64;
        events.blocking_send(QueryEvent::Rows { rows: page }).is_ok()
    };

    if let Some(row) = held.take() {
        match pager.push(row) {
            Paged::Continue => {}
            Paged::Page(page) => {
                if !send(page) {
                    return (delivered, Ok(false), false);
                }
            }
            Paged::Overflow(row) => {
                *held = Some(row);
                return (0, Ok(false), true);
            }
        }
    }
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let row = (0..width).map(|i| value(row.get_ref_unwrap(i))).collect();
                match pager.push(row) {
                    Paged::Continue => {}
                    Paged::Page(page) => {
                        if !send(page) {
                            return (delivered, Ok(false), false);
                        }
                        if requested.load(Ordering::SeqCst) {
                            return (delivered, Ok(true), false);
                        }
                    }
                    Paged::Overflow(row) => {
                        let listening = send(pager.finish());
                        *held = Some(row);
                        return (delivered, Ok(false), listening);
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                // Rows stepped before an interrupt are valid; deliver them so
                // the count matches what the caller received.
                if e.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) {
                    send(pager.finish());
                }
                return (delivered, Err(e), false);
            }
        }
    }
    send(pager.finish());
    (delivered, Ok(requested.load(Ordering::SeqCst)), false)
}

fn value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Value::Text(text.to_owned()),
            Err(_) => Value::Bytes(bytes.to_vec()),
        },
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}

/// Quotes an identifier for interpolation into SQL.
fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
