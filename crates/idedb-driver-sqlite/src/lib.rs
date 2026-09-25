//! SQLite sessions built on `rusqlite` with a bundled SQLite.
//!
//! rusqlite is blocking, so all database work runs on tokio's blocking pool.
//! Result pages travel back to the async caller through a small bounded
//! channel: `emit` only ever runs on the caller's task, and the backpressure
//! keeps a fast statement from buffering its whole result in memory.

mod apply;
mod introspect;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectionParams, Engine, Error, QueryEvent, Result, Row, RowChange,
    SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, TableRef, Value,
};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, InterruptHandle};
use tokio::sync::mpsc;

/// How many VM instructions run between checks of the cancel flag.
const PROGRESS_INTERVAL: i32 = 1000;

/// Pages buffered between the blocking worker and the caller.
const PAGES_IN_FLIGHT: usize = 2;

pub struct SqliteSession {
    conn: Arc<Mutex<Connection>>,
    canceller: SqliteCanceller,
    info: ServerInfo,
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
        Ok(Self { conn: Arc::new(Mutex::new(conn)), canceller, info })
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

    async fn execute(&mut self, sql: &str, page_size: usize, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        let started = Instant::now();
        let state = self.canceller.state.clone();
        state.begin();

        let (tx, mut rx) = mpsc::channel(PAGES_IN_FLIGHT);
        let conn = self.conn.clone();
        let sql = sql.to_owned();
        let worker = tokio::task::spawn_blocking(move || {
            let outcome = {
                let conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
                // SQLite knows exactly whether a transaction is open: it is
                // in autocommit mode otherwise.
                (run(&conn, &sql, page_size.max(1), &state.requested, &tx), !conn.is_autocommit())
            };
            state.finish();
            outcome
        });

        while let Some(event) = rx.recv().await {
            emit(event);
        }
        let elapsed_ms = || started.elapsed().as_millis() as u64;

        emit(match worker.await {
            Ok((Outcome { row_count, result: Ok(cancelled) }, in_transaction)) => {
                QueryEvent::Done { row_count, elapsed_ms: elapsed_ms(), cancelled, in_transaction }
            }
            Ok((Outcome { row_count, result: Err(e) }, in_transaction))
                if e.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) =>
            {
                QueryEvent::Done { row_count, elapsed_ms: elapsed_ms(), cancelled: true, in_transaction }
            }
            Ok((Outcome { result: Err(e), .. }, in_transaction)) => error_event(&e, in_transaction),
            Err(e) => QueryEvent::Error {
                message: format!("sqlite worker failed: {e}"),
                position: None,
                in_transaction: false,
            },
        });
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        self.blocking(introspect::schemas).await
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        let schema = schema.to_owned();
        self.blocking(move |conn| introspect::schema_model(conn, &schema)).await
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
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
        Ok(())
    }
}

/// What a statement did: rows emitted (or changed, for statements without a
/// result set) and whether it ended by cancellation.
struct Outcome {
    row_count: u64,
    result: rusqlite::Result<bool>,
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

/// Runs one statement on the blocking pool, sending events through `tx`.
/// Stops early when the receiver is gone.
fn run(
    conn: &Connection,
    sql: &str,
    page_size: usize,
    requested: &AtomicBool,
    tx: &mpsc::Sender<QueryEvent>,
) -> Outcome {
    let fail = |e| Outcome { row_count: 0, result: Err(e) };
    let mut statement = match conn.prepare(sql) {
        Ok(statement) => statement,
        Err(e) => return fail(e),
    };

    if statement.column_count() == 0 {
        // `changes()` keeps the count of the last INSERT/UPDATE/DELETE, so a
        // DDL statement would report a stale number. `total_changes()` only
        // moves when this statement changed rows.
        let before = conn.total_changes();
        return match statement.execute([]) {
            Ok(_) => {
                let changed = if conn.total_changes() == before { 0 } else { conn.changes() };
                Outcome { row_count: changed, result: Ok(false) }
            }
            Err(e) => fail(e),
        };
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
    if tx.blocking_send(QueryEvent::Columns { columns }).is_err() {
        return Outcome { row_count: 0, result: Ok(false) };
    }

    let mut rows = match statement.query([]) {
        Ok(rows) => rows,
        Err(e) => return fail(e),
    };
    let mut page: Vec<Row> = Vec::with_capacity(page_size);
    let mut total = 0u64;
    let send = |page: &mut Vec<Row>| {
        let rows = std::mem::replace(page, Vec::with_capacity(page_size));
        tx.blocking_send(QueryEvent::Rows { rows }).is_ok()
    };

    loop {
        match rows.next() {
            Ok(Some(row)) => {
                page.push((0..width).map(|i| value(row.get_ref_unwrap(i))).collect());
                total += 1;
                if page.len() == page_size {
                    if !send(&mut page) {
                        return Outcome { row_count: total, result: Ok(false) };
                    }
                    if requested.load(Ordering::SeqCst) {
                        return Outcome { row_count: total, result: Ok(true) };
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                // Rows stepped before an interrupt are valid; deliver them so
                // `row_count` matches what the caller received.
                if e.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) && !page.is_empty() {
                    send(&mut page);
                }
                return Outcome { row_count: total, result: Err(e) };
            }
        }
    }

    if !page.is_empty() {
        send(&mut page);
    }
    Outcome { row_count: total, result: Ok(requested.load(Ordering::SeqCst)) }
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
