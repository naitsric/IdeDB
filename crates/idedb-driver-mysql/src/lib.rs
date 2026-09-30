//! MySQL sessions built on `mysql_async`.
//!
//! Statements run over the text protocol with `query_iter`, which streams
//! rows off the socket as the server sends them, so arbitrarily large
//! results are paged to the UI without ever sitting fully in memory.
//!
//! MySQL has no out-of-band cancel request: cancelling runs
//! `KILL QUERY <id>` over a separate short-lived connection.
//!
//! Whether the user has a transaction open comes from the server's own
//! status flags (`SERVER_STATUS_IN_TRANS`, `SERVER_STATUS_AUTOCOMMIT`),
//! which every OK packet and result-set terminator carries.
//!
//! A read with a fetch limit pauses by simply not reading on: the rest of
//! the result stays pending on the connection (mysql_async keeps it in the
//! `Conn`, not in the borrowed `QueryResult`), and the server waits on the
//! socket holding the statement's locks. Nothing else can use the connection
//! meanwhile, so anything the session does first reads out a small rest, or
//! stops the server with `KILL QUERY` and reads to the error it causes.

mod apply;
mod decode;
mod introspect;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectOptions, ConnectionParams, Engine, Error, Fetch, NO_OPEN_RESULT, Paged,
    Pager, QueryEvent, Result, Row, RowChange, SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, SslMode,
    TableRef,
};
use mysql_async::consts::StatusFlags;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, QueryResult, SslOpts, TextProtocol};
use tokio::sync::Notify;

use crate::decode::Kind;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `ER_QUERY_INTERRUPTED`: the running statement was hit by `KILL QUERY`.
const QUERY_INTERRUPTED: u16 = 1317;
/// `ER_NO_SUCH_THREAD`: the connection to kill is already gone.
const NO_SUCH_THREAD: u16 = 1094;
/// `ER_UNSUPPORTED_PS`: the statement cannot be prepared, only run.
const UNSUPPORTED_PS: u16 = 1295;
/// Rows of a paused result read and dropped before closing it with
/// `KILL QUERY` instead: cheaper than a second connection when little is
/// left, bounded when a lot is.
const DRAIN_BUDGET: usize = 10_000;
/// Makes every transaction the session runs read only, for
/// [`ConnectOptions::read_only`]. Run as an init command of the connection
/// options, so every connection made from them gets it: the session's, the
/// ones that replace it on reconnect, and the short-lived ones that send
/// `KILL QUERY` (which read-only mode does not stop).
const READ_ONLY: &str = "SET SESSION TRANSACTION READ ONLY";

pub struct MySqlSession {
    conn: Conn,
    opts: Opts,
    server: ServerInfo,
    /// Current database, as the server reported it after the last statement
    /// (so a `USE` typed in a console counts). Restored when a broken
    /// connection is replaced.
    database: Option<String>,
    cancel: Arc<CancelState>,
    /// Set after a fatal (I/O or protocol) error; the next call reconnects.
    broken: bool,
    /// Whether the user had a transaction open after the last statement.
    in_transaction: bool,
    /// A read paused at its fetch limit; its rows are still pending on `conn`.
    open: Option<OpenResult>,
}

struct OpenResult {
    /// How to decode each column of the pending rows.
    kinds: Vec<Kind>,
    /// The row read past the last fetch's limit, first in line for the next.
    held: Option<Row>,
}

/// What reading the pending result did.
struct Pumped {
    rows: u64,
    has_more: bool,
}

/// Cancels the statement currently running on a session. Cloneable so it can
/// be used while the session itself is busy executing.
#[derive(Clone)]
pub struct MySqlCanceller {
    state: Arc<CancelState>,
}

struct CancelState {
    opts: Opts,
    /// Server-side id of the session's connection, the target of `KILL QUERY`.
    connection_id: AtomicU32,
    /// Checked between pages; reset when a statement starts.
    requested: AtomicBool,
    guard: Mutex<Guard>,
    /// Signalled whenever an in-flight kill finishes.
    kill_done: Notify,
}

#[derive(Default)]
struct Guard {
    running: bool,
    in_flight_kills: usize,
}

impl Canceller for MySqlCanceller {
    async fn cancel(&self) -> Result<()> {
        {
            let mut guard = self.state.guard.lock().unwrap();
            // Only the first request per statement sends a KILL: repeated
            // or late ones must never hit whatever runs next.
            if !guard.running || self.state.requested.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            guard.in_flight_kills += 1;
        }
        let outcome = self.state.kill_query().await;
        self.state.guard.lock().unwrap().in_flight_kills -= 1;
        self.state.kill_done.notify_waiters();
        outcome
    }
}

impl CancelState {
    async fn kill_query(&self) -> Result<()> {
        let id = self.connection_id.load(Ordering::SeqCst);
        let kill = async {
            let mut conn = Conn::new(self.opts.clone()).await?;
            let outcome = conn.query_drop(format!("KILL QUERY {id}")).await;
            let _ = conn.disconnect().await;
            match outcome {
                Err(mysql_async::Error::Server(e)) if e.code == NO_SUCH_THREAD => Ok(()),
                other => other,
            }
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, kill).await {
            Ok(outcome) => outcome.map_err(|e| Error::Query(format_error(&e))),
            Err(_) => Err(Error::Query("timed out sending the cancel request".into())),
        }
    }

    fn begin(&self) {
        self.guard.lock().unwrap().running = true;
        self.requested.store(false, Ordering::SeqCst);
    }

    /// Marks the session idle and waits for kills already on their way, so
    /// a late `KILL QUERY` can never land on the next statement.
    async fn end(&self) {
        // Let cancels spawned during the statement (but not yet polled) run
        // now: they will observe the session idle and do nothing.
        tokio::task::yield_now().await;
        self.guard.lock().unwrap().running = false;
        loop {
            let notified = self.kill_done.notified();
            if self.guard.lock().unwrap().in_flight_kills == 0 {
                break;
            }
            notified.await;
        }
    }
}

impl MySqlSession {
    pub async fn connect(params: &ConnectionParams, password: Option<&str>) -> Result<Self> {
        Self::connect_with(params, password, ConnectOptions::default()).await
    }

    /// Like [`connect`](Self::connect), set up as `options` asks.
    pub async fn connect_with(
        params: &ConnectionParams,
        password: Option<&str>,
        options: ConnectOptions,
    ) -> Result<Self> {
        let base = OptsBuilder::default()
            .ip_or_hostname(if params.host.is_empty() {
                "localhost"
            } else {
                &params.host
            })
            .tcp_port(params.port_or_default())
            .user((!params.user.is_empty()).then_some(params.user.as_str()))
            .pass(password)
            .db_name((!params.database.is_empty()).then_some(params.database.as_str()))
            // Asking a local server for its unix socket would find the
            // container's path, not one on this host.
            .prefer_socket(false)
            .init(if options.read_only { vec![READ_ONLY] } else { vec![] });

        // libpq semantics: prefer/require encrypt without verifying.
        let unverified = SslOpts::default()
            .with_danger_accept_invalid_certs(true)
            .with_danger_skip_domain_validation(true);
        let (conn, opts) = match params.ssl_mode {
            SslMode::Disable => open(base.ssl_opts(None::<SslOpts>)).await?,
            SslMode::Require => open(base.ssl_opts(unverified)).await?,
            SslMode::VerifyFull => open(base.ssl_opts(SslOpts::default())).await?,
            SslMode::Prefer => match open(base.clone().ssl_opts(unverified)).await {
                Ok(opened) => opened,
                // Credentials and similar server errors would fail again in
                // plain text; only transport and TLS failures fall back.
                Err(OpenError::Server(message)) => return Err(Error::Connect(message)),
                Err(OpenError::Transport(_)) => open(base.ssl_opts(None::<SslOpts>)).await?,
            },
        };

        let mut session = Self {
            cancel: Arc::new(CancelState {
                opts: opts.clone(),
                connection_id: AtomicU32::new(conn.id()),
                requested: AtomicBool::new(false),
                guard: Mutex::new(Guard::default()),
                kill_done: Notify::new(),
            }),
            conn,
            opts,
            server: ServerInfo {
                engine: Engine::Mysql,
                version: String::new(),
                default_schema: None,
            },
            database: None,
            broken: false,
            in_transaction: false,
            open: None,
        };
        let (version, database): (String, Option<String>) = session
            .conn
            .query_first("select version(), database()")
            .await
            .map_err(|e| Error::Connect(format_error(&e)))?
            .unwrap_or_default();
        session.server.version = version;
        session.server.default_schema = database.clone();
        session.database = database;
        Ok(session)
    }

    /// Asks the server for the current database and whether the user has a
    /// transaction open (`BEGIN`, or autocommit turned off). The status flags
    /// come from the terminator of this very query, so they describe the
    /// session as it is now.
    async fn probe(&mut self) -> Result<(Option<String>, bool), mysql_async::Error> {
        let database: Option<Option<String>> = self.conn.query_first("select database()").await?;
        let flags = self
            .conn
            .last_ok_packet()
            .map(|ok| ok.status_flags())
            .ok_or_else(|| mysql_async::Error::Other("the server reported no session status".into()))?;
        let open = flags.contains(StatusFlags::SERVER_STATUS_IN_TRANS)
            || !flags.contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT);
        Ok((database.flatten(), open))
    }

    /// Refreshes `database` and `in_transaction` from the server.
    async fn refresh_state(&mut self) -> Result<bool, mysql_async::Error> {
        let (database, open) = self.probe().await?;
        self.database = database;
        self.in_transaction = open;
        Ok(open)
    }

    /// Replaces a connection that failed fatally, keeping the selected
    /// database. Read-only mode comes back with the options' init command.
    async fn reconnect(&mut self) -> Result<(), mysql_async::Error> {
        let opts = OptsBuilder::from_opts(self.opts.clone()).db_name(self.database.clone());
        let conn = Conn::new(opts).await?;
        self.cancel.connection_id.store(conn.id(), Ordering::SeqCst);
        self.conn = conn;
        self.broken = false;
        self.in_transaction = false;
        // Whatever was paused went with the old connection.
        self.open = None;
        Ok(())
    }

    /// Replaces a lost connection and says what the user lost with it: the
    /// open transaction (rolled back by the server) or other session state.
    async fn recover(&mut self) -> String {
        let lost_transaction = self.in_transaction;
        match self.reconnect().await {
            Ok(()) => reconnected_notice(lost_transaction, self.database.as_deref()),
            Err(e) => format!("The connection to the server was lost and reconnecting failed: {}", format_error(&e)),
        }
    }

    async fn run(
        &mut self,
        sql: &str,
        fetch: Fetch,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<Pumped, mysql_async::Error> {
        let result = self.conn.query_iter(sql).await?;

        let Some(columns) = result.columns().filter(|c| !c.is_empty()) else {
            let affected = result.affected_rows();
            result.drop_result().await?;
            return Ok(Pumped { rows: affected, has_more: false });
        };

        emit(QueryEvent::Columns {
            columns: columns
                .iter()
                .map(|c| Column {
                    name: c.name_str().into_owned(),
                    type_name: decode::type_name(c),
                })
                .collect(),
        });
        let kinds: Vec<Kind> = columns.iter().map(Kind::of).collect();
        // The rows stay pending on the connection; `pump` reads them.
        drop(result);
        self.open = Some(OpenResult { kinds, held: None });
        self.pump(fetch, emit).await
    }

    /// Reads the pending result up to the fetch limit (to its end without
    /// one). Leaves it pending when the limit stops the read; otherwise
    /// finishes it, and any further result sets, so the connection is clean.
    /// A cancel has sent `KILL QUERY`, which ends the stream server-side:
    /// what is still in flight is read and dropped.
    async fn pump(&mut self, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) -> Result<Pumped, mysql_async::Error> {
        let Some(open) = self.open.as_mut() else { return Ok(Pumped { rows: 0, has_more: false }) };
        let kinds = open.kinds.clone();
        let held = open.held.take();
        let mut pager = Pager::new(fetch);
        let mut emitted = 0u64;
        let mut send = |rows: Vec<Row>, emit: &mut (dyn FnMut(QueryEvent) + Send)| {
            if !rows.is_empty() {
                emitted += rows.len() as u64;
                emit(QueryEvent::Rows { rows });
            }
        };
        if let Some(row) = held {
            match pager.push(row) {
                Paged::Continue => {}
                Paged::Page(rows) => send(rows, emit),
                Paged::Overflow(row) => {
                    open.held = Some(row);
                    return Ok(Pumped { rows: 0, has_more: true });
                }
            }
        }

        let requested = &self.cancel.requested;
        let mut result = QueryResult::<'_, '_, TextProtocol>::new(&mut self.conn);
        loop {
            let row = match result.next().await {
                Ok(Some(row)) => decode_row(row, &kinds),
                Ok(None) => break,
                Err(e) => {
                    self.open = None;
                    return Err(e);
                }
            };
            match pager.push(row) {
                Paged::Continue => {}
                Paged::Page(rows) => {
                    if requested.load(Ordering::SeqCst) {
                        while let Ok(Some(_)) = result.next().await {}
                        self.open = None;
                        return Ok(Pumped { rows: emitted, has_more: false });
                    }
                    send(rows, emit);
                }
                Paged::Overflow(row) => {
                    send(pager.finish(), emit);
                    if let Some(open) = self.open.as_mut() {
                        open.held = Some(row);
                    }
                    return Ok(Pumped { rows: emitted, has_more: true });
                }
            }
        }
        if !requested.load(Ordering::SeqCst) {
            send(pager.finish(), emit);
        }
        self.open = None;
        result.drop_result().await?;
        Ok(Pumped { rows: emitted, has_more: false })
    }

    /// Releases a paused result (see [`Session::close_result`]): reads out a
    /// small rest, or stops the server with `KILL QUERY` and reads to the
    /// error it causes. Then asks the server what state the finished
    /// statement left the session in.
    async fn close_open(&mut self) {
        if self.open.take().is_none() || self.broken {
            return;
        }
        let cancel = self.cancel.clone();
        let mut killed = false;
        let mut read = 0usize;
        let mut result = QueryResult::<'_, '_, TextProtocol>::new(&mut self.conn);
        let ended = loop {
            match result.next().await {
                Ok(Some(_)) => {
                    read += 1;
                    if read == DRAIN_BUDGET {
                        killed = true;
                        // Best effort: without it the rest is read out instead.
                        let _ = cancel.kill_query().await;
                    }
                }
                Ok(None) => break result.drop_result().await,
                Err(e) => break Err(e),
            }
        };
        if ended.as_ref().is_err_and(connection_lost) {
            self.broken = true;
            return;
        }
        // A KILL that raced the end of the result may still be pending on the
        // session: let a no-op absorb it rather than the user's next statement.
        if killed && self.conn.query_drop("DO 0").await.as_ref().is_err_and(connection_lost) {
            self.broken = true;
            return;
        }
        if self.refresh_state().await.is_err() {
            self.broken = true;
        }
    }
}

fn decode_row(row: mysql_async::Row, kinds: &[Kind]) -> Row {
    row.unwrap_raw()
        .into_iter()
        .zip(kinds)
        .map(|(value, kind)| value.map_or(idedb_core::Value::Null, |v| kind.decode(v)))
        .collect()
}

impl Session for MySqlSession {
    type Canceller = MySqlCanceller;

    fn canceller(&self) -> MySqlCanceller {
        MySqlCanceller {
            state: self.cancel.clone(),
        }
    }

    fn server_info(&self) -> &ServerInfo {
        &self.server
    }

    async fn execute(
        &mut self,
        sql: &str,
        fetch: Fetch,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) {
        self.close_open().await;
        // A statement never runs on a connection that replaced a lost one
        // without the user hearing about it: what was lost (an open
        // transaction, temporary tables, variables) could change its meaning.
        if self.broken {
            let notice = self.recover().await;
            emit(QueryEvent::Error {
                message: format!("{notice} The statement was not run; run it again."),
                position: None,
                in_transaction: false,
            });
            return;
        }

        self.cancel.begin();
        let started = Instant::now();
        let fetch = Fetch { page_size: fetch.page_size.max(1), ..fetch };
        let outcome = self.run(sql, fetch, emit).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.cancel.end().await;
        self.finish_call(outcome, elapsed_ms, Some(sql), emit).await;
    }

    async fn fetch_more(&mut self, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        if self.open.is_none() {
            emit(QueryEvent::Error { message: NO_OPEN_RESULT.into(), position: None, in_transaction: self.in_transaction });
            return;
        }
        if self.broken {
            let notice = self.recover().await;
            emit(QueryEvent::Error { message: format!("{NO_OPEN_RESULT}\n{notice}"), position: None, in_transaction: false });
            return;
        }
        self.cancel.begin();
        let started = Instant::now();
        let fetch = Fetch { page_size: fetch.page_size.max(1), ..fetch };
        let outcome = self.pump(fetch, emit).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.cancel.end().await;
        self.finish_call(outcome, elapsed_ms, None, emit).await;
    }

    async fn close_result(&mut self) {
        self.close_open().await;
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        self.close_open().await;
        self.ensure_connected().await?;
        introspect::schemas(&mut self.conn)
            .await
            .map_err(|e| self.query_error(e))
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        self.close_open().await;
        self.ensure_connected().await?;
        introspect::introspect(&mut self.conn, schema)
            .await
            .map_err(|e| self.query_error(e))
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        self.close_open().await;
        // Changes meant for a transaction the lost connection took along
        // must not be committed on their own on a fresh one.
        if self.broken {
            let notice = self.recover().await;
            return Err(Error::Connect(format!("{notice} Nothing was saved; submit again.")));
        }
        let inside = self.refresh_state().await.map_err(|e| self.query_error(e))?;
        apply::apply(&mut self.conn, table, changes, inside)
            .await
            .map_err(|e| self.query_error(e))
    }

    /// A server-side prepare (`COM_STMT_PREPARE`), closed right away: MySQL
    /// parses the statement and resolves its tables and columns, and runs
    /// nothing. With a schema, the session switches to that database first
    /// (it only ever serves checks and introspection, which name schemas
    /// explicitly).
    async fn check(&mut self, sql: &str, schema: Option<&str>) -> Result<Option<SqlProblem>> {
        self.close_open().await;
        self.ensure_connected().await?;
        if let Some(schema) = schema {
            self.set_schema(schema).await?;
        }
        match self.conn.prep(sql).await {
            Ok(statement) => {
                self.conn.close(statement).await.map_err(|e| self.query_error(e))?;
                Ok(None)
            }
            Err(mysql_async::Error::Server(e)) if e.code == UNSUPPORTED_PS => Ok(None),
            Err(e @ mysql_async::Error::Server(_)) => {
                let message = format_error(&e);
                Ok(Some(SqlProblem { position: near_position(sql, &message), message }))
            }
            Err(e) => Err(self.query_error(e)),
        }
    }

    async fn set_schema(&mut self, schema: &str) -> Result<()> {
        self.close_open().await;
        self.ensure_connected().await?;
        if self.database.as_deref() == Some(schema) {
            return Ok(());
        }
        self.conn
            .query_drop(format!("USE {}", apply::quote(schema)))
            .await
            .map_err(|e| self.query_error(e))?;
        self.database = Some(schema.to_owned());
        Ok(())
    }
}

/// MySQL locates syntax errors only in the message text, as `near '<the
/// statement from the error on>' at line N` (cut at 80 characters). Finds
/// that text in the statement, from line N on, as a char offset.
fn near_position(sql: &str, message: &str) -> Option<u32> {
    const NEAR: &str = " near '";
    const AT_LINE: &str = "' at line ";
    let start = message.find(NEAR)? + NEAR.len();
    let end = message.rfind(AT_LINE)?;
    let snippet = message.get(start..end)?;
    let line: usize = message[end + AT_LINE.len()..].trim().parse().ok()?;
    let line_start: usize = sql.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum();
    let byte = if snippet.is_empty() { sql.len() } else { line_start + sql.get(line_start..)?.find(snippet)? };
    Some(sql[..byte].chars().count() as u32)
}

impl MySqlSession {
    /// Reports how a statement or a fetch ended, after bringing the session
    /// state up to date. `sql` locates syntax errors (statements only).
    async fn finish_call(
        &mut self,
        outcome: Result<Pumped, mysql_async::Error>,
        elapsed_ms: u64,
        sql: Option<&str>,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) {
        if let Err(e) = &outcome {
            self.broken |= connection_lost(e);
        }
        // Any statement may have changed the database or the transaction
        // state (USE, BEGIN, COMMIT, a deadlock rollback, ...): ask the
        // server, unless a paused result still holds the connection (only
        // reads pause, and a read changes neither).
        if !self.broken && self.open.is_none() && self.refresh_state().await.is_err() {
            self.broken = true;
        }
        // Reconnect right away so the next statement runs normally; the
        // error below tells the user what the lost connection took with it.
        let lost = if self.broken { Some(self.recover().await) } else { None };
        let in_transaction = self.in_transaction;

        // An interrupted SLEEP() returns normally, so the request flag, not
        // the outcome, decides whether the statement was cancelled.
        let cancelled = self.cancel.requested.load(Ordering::SeqCst);
        if let Some(notice) = lost {
            let cause = outcome.err().map(|e| format_error(&e)).unwrap_or_default();
            emit(QueryEvent::Error {
                message: format!("{cause}\n{notice} The statement may or may not have completed."),
                position: None,
                in_transaction,
            });
            return;
        }
        emit(match outcome {
            Ok(pumped) => QueryEvent::Done {
                row_count: pumped.rows,
                elapsed_ms,
                cancelled,
                has_more: pumped.has_more,
                in_transaction,
            },
            Err(e) if cancelled || server_code(&e) == Some(QUERY_INTERRUPTED) => QueryEvent::Done {
                row_count: 0,
                elapsed_ms,
                cancelled: true,
                has_more: false,
                in_transaction,
            },
            Err(e) => {
                let message = format_error(&e);
                QueryEvent::Error {
                    position: sql.and_then(|sql| near_position(sql, &message)),
                    message,
                    in_transaction,
                }
            }
        });
    }

    async fn ensure_connected(&mut self) -> Result<()> {
        if self.broken {
            self.reconnect()
                .await
                .map_err(|e| Error::Connect(format_error(&e)))?;
        }
        Ok(())
    }

    fn query_error(&mut self, e: mysql_async::Error) -> Error {
        self.broken |= connection_lost(&e);
        Error::Query(format_error(&e))
    }
}

/// `ER_SERVER_SHUTDOWN`, `ER_CONNECTION_KILLED`, `ER_CLIENT_INTERACTION_TIMEOUT`:
/// the server ended the connection and said so.
const CONNECTION_ENDED: [u16; 3] = [1053, 1927, 4031];

/// Whether the error means the connection is gone (I/O, protocol, or the
/// server ending it), rather than a statement failing.
fn connection_lost(e: &mysql_async::Error) -> bool {
    e.is_fatal() || server_code(e).is_some_and(|code| CONNECTION_ENDED.contains(&code))
}

fn reconnected_notice(lost_transaction: bool, database: Option<&str>) -> String {
    let restored = database.map(|db| format!(" (current database `{db}` restored)")).unwrap_or_default();
    let lost = if lost_transaction {
        "The transaction that was open was rolled back by the server: nothing in it was committed."
    } else {
        "Session state such as temporary tables and user variables was reset."
    };
    format!("The connection to the server was lost and has been re-established{restored}. {lost}")
}

enum OpenError {
    /// The server answered and refused (bad credentials, unknown database, ...).
    Server(String),
    /// The connection or TLS handshake itself failed.
    Transport(String),
}

impl From<OpenError> for Error {
    fn from(e: OpenError) -> Self {
        match e {
            OpenError::Server(message) | OpenError::Transport(message) => Error::Connect(message),
        }
    }
}

async fn open(builder: OptsBuilder) -> Result<(Conn, Opts), OpenError> {
    let opts = Opts::from(builder);
    match tokio::time::timeout(CONNECT_TIMEOUT, Conn::new(opts.clone())).await {
        Ok(Ok(conn)) => Ok((conn, opts)),
        Ok(Err(e @ mysql_async::Error::Server(_))) => Err(OpenError::Server(format_error(&e))),
        Ok(Err(e)) => Err(OpenError::Transport(format_error(&e))),
        Err(_) => Err(OpenError::Transport("timed out connecting".into())),
    }
}

fn server_code(e: &mysql_async::Error) -> Option<u16> {
    match e {
        mysql_async::Error::Server(s) => Some(s.code),
        _ => None,
    }
}

/// Server errors in the familiar `mysql` CLI shape; client errors as-is.
fn format_error(e: &mysql_async::Error) -> String {
    match e {
        mysql_async::Error::Server(s) => format!("ERROR {} ({}): {}", s.code, s.state, s.message),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::near_position;

    const SYNTAX: &str = "ERROR 1064 (42000): You have an error in your SQL syntax; check the manual that \
                          corresponds to your MySQL server version for the right syntax to use near ";

    #[test]
    fn locates_the_near_text_from_its_line_in_chars() {
        let sql = "select 'ñ' from t;\nselect from t";
        assert_eq!(near_position(sql, &format!("{SYNTAX}'from t' at line 2")), Some(26));
        // The same text on an earlier line is not the one MySQL means.
        assert_eq!(near_position(sql, &format!("{SYNTAX}'from t' at line 1")), Some(11));
    }

    #[test]
    fn places_an_error_at_the_end_when_the_near_text_is_empty() {
        assert_eq!(near_position("select 1 +", &format!("{SYNTAX}'' at line 1")), Some(10));
    }

    #[test]
    fn has_no_position_without_near_text() {
        assert_eq!(near_position("select * from t", "ERROR 1146 (42S02): Table 'idedb.t' doesn't exist"), None);
    }
}
