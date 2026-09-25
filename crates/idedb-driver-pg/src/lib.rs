//! PostgreSQL sessions built on `tokio-postgres`.
//!
//! Result sets are paged `page_size` rows at a time, so arbitrarily large
//! results never sit fully in memory:
//! - outside a transaction block, through a protocol-level portal inside a
//!   transaction of the driver's own, which works for any statement without
//!   rewriting it;
//! - inside a transaction the user opened, through a SQL cursor in a
//!   savepoint, so the driver never commits or rolls back the user's
//!   transaction. Statements a cursor cannot run (`INSERT ... RETURNING`,
//!   `EXPLAIN`, ...) stream instead.
//!
//! Whether the user has a transaction block open is asked of the server
//! (see [`PgSession::transaction_open`]), never guessed from the SQL.

mod apply;
mod decode;
mod introspect;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{TryStreamExt, pin_mut};
use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectionParams, Engine, Error, QueryEvent, Result, Row,
    RowChange, SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, SslMode, TableRef,
};
use postgres_native_tls::MakeTlsConnector;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::types::ToSql;
use tokio_postgres::{CancelToken, Client, Config, SimpleQueryMessage, Statement};

use crate::decode::Cell;

pub struct PgSession {
    client: Client,
    /// How to connect again when the connection is lost.
    config: Config,
    cancel: Arc<CancelState>,
    server: ServerInfo,
    connection: JoinHandle<()>,
    /// Whether the user had a transaction block open after the last statement.
    in_transaction: bool,
    /// Head of the search path set with `set_schema`, restored on reconnect.
    schema: Option<String>,
}

/// Cancels the statement currently running on a session. Cloneable so it can
/// be used while the session itself is busy executing.
#[derive(Clone)]
pub struct PgCanceller {
    state: Arc<CancelState>,
}

struct CancelState {
    token: Mutex<CancelToken>,
    tls: MakeTlsConnector,
    /// Checked between pages; reset when a statement starts.
    requested: AtomicBool,
    guard: Mutex<Guard>,
    /// Signalled whenever an in-flight cancel request finishes.
    request_done: Notify,
}

#[derive(Default)]
struct Guard {
    running: bool,
    in_flight: usize,
}

impl Canceller for PgCanceller {
    async fn cancel(&self) -> Result<()> {
        let token = {
            let mut guard = self.state.guard.lock().unwrap();
            // Only a running statement is cancelled, and only once: an idle
            // session has nothing to cancel, and an extra or late request
            // could land on whatever runs next.
            if !guard.running || self.state.requested.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            guard.in_flight += 1;
            self.state.token.lock().unwrap().clone()
        };
        // The flag alone stops paging between fetches, when nothing is
        // running on the server for the request to hit.
        let outcome = token.cancel_query(self.state.tls.clone()).await.map_err(|e| Error::Query(e.to_string()));
        self.state.guard.lock().unwrap().in_flight -= 1;
        self.state.request_done.notify_waiters();
        outcome
    }
}

impl CancelState {
    fn begin(&self) {
        self.guard.lock().unwrap().running = true;
        self.requested.store(false, Ordering::SeqCst);
    }

    /// Marks the session idle and waits for cancel requests already on their
    /// way. The server ignores a cancel that arrives while it waits for the
    /// next command, so none can hit the statement that runs next.
    async fn end(&self) {
        // Let cancels spawned during the statement (but not yet polled) run
        // now: they will see the session idle and do nothing.
        tokio::task::yield_now().await;
        self.guard.lock().unwrap().running = false;
        loop {
            let notified = self.request_done.notified();
            if self.guard.lock().unwrap().in_flight == 0 {
                break;
            }
            notified.await;
        }
    }
}

/// What a statement did.
struct Ran {
    row_count: u64,
    cancelled: bool,
    /// Transaction state after the statement, when it is known without
    /// asking the server (a read cannot open or close a transaction block).
    in_transaction: Option<bool>,
}

impl PgSession {
    pub async fn connect(params: &ConnectionParams, password: Option<&str>) -> Result<Self> {
        let mut config = Config::new();
        config
            .host(if params.host.is_empty() { "localhost" } else { &params.host })
            .port(params.port_or_default())
            .user(&params.user)
            .dbname(if params.database.is_empty() { "postgres" } else { &params.database })
            .application_name("idedb")
            .connect_timeout(Duration::from_secs(10))
            .ssl_mode(match params.ssl_mode {
                SslMode::Disable => tokio_postgres::config::SslMode::Disable,
                SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
                SslMode::Require | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
            });
        if let Some(password) = password {
            config.password(password);
        }
        let tls = tls_connector(params.ssl_mode)?;
        let (client, connection) = open(&config, &tls).await.map_err(|e| Error::Connect(format_error(&e)))?;

        let row = client
            .query_one("select current_setting('server_version'), current_schema()", &[])
            .await
            .map_err(|e| Error::Connect(format_error(&e)))?;
        let server = ServerInfo {
            engine: Engine::Postgres,
            version: row.get(0),
            default_schema: row.get(1),
        };
        let cancel = Arc::new(CancelState {
            token: Mutex::new(client.cancel_token()),
            tls,
            requested: AtomicBool::new(false),
            guard: Mutex::new(Guard::default()),
            request_done: Notify::new(),
        });
        Ok(Self { client, config, cancel, server, connection, in_transaction: false, schema: None })
    }

    /// Replaces a lost connection, restoring the search path the console chose.
    async fn reconnect(&mut self) -> Result<(), tokio_postgres::Error> {
        let (client, connection) = open(&self.config, &self.cancel.tls).await?;
        if let Some(schema) = &self.schema {
            client.batch_execute(&format!("set search_path to {}", search_path(schema))).await?;
        }
        *self.cancel.token.lock().unwrap() = client.cancel_token();
        self.connection.abort();
        self.client = client;
        self.connection = connection;
        self.in_transaction = false;
        Ok(())
    }

    /// Replaces a lost connection and says what the user lost with it: the
    /// open transaction (rolled back by the server) or other session state.
    async fn recover(&mut self) -> String {
        let lost_transaction = self.in_transaction;
        match self.reconnect().await {
            Ok(()) => reconnected_notice(lost_transaction, self.schema.as_deref()),
            Err(e) => format!("The connection to the server was lost and reconnecting failed: {}", format_error(&e)),
        }
    }

    /// Reconnects quietly, for the explorer's reads (introspection, checks).
    async fn ensure_connected(&mut self) -> Result<()> {
        if self.client.is_closed() {
            self.reconnect().await.map_err(|e| Error::Connect(format_error(&e)))?;
        }
        Ok(())
    }

    /// Whether a transaction block is open, asked of the server itself.
    ///
    /// `statement_timestamp()` equals `transaction_timestamp()` exactly
    /// during the first statement of a transaction (documented Postgres
    /// behavior), which this probe is only when no block is open. It runs
    /// over the simple query protocol, where a statement and its implicit
    /// transaction share one start timestamp. In an aborted block every
    /// statement fails with 25P02, which also means "open".
    async fn transaction_open(&self) -> Result<bool, tokio_postgres::Error> {
        let probe = "select pg_catalog.transaction_timestamp() = pg_catalog.statement_timestamp()";
        match self.client.simple_query(probe).await {
            Ok(messages) => Ok(messages
                .iter()
                .any(|m| matches!(m, SimpleQueryMessage::Row(row) if row.get(0) == Some("f")))),
            Err(e) if e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION) => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// Closes a transaction the driver itself opened, after the statement it
    /// served failed. A dropped `Transaction` already queued a ROLLBACK; this
    /// confirms with the server that it took, and retries if a cancel
    /// request interrupted it. Never called while the user has a block open.
    async fn end_own_transaction(&self) {
        for _ in 0..3 {
            match self.transaction_open().await {
                Ok(true) => {
                    let _ = self.client.batch_execute("rollback").await;
                }
                Ok(false) | Err(_) => return,
            }
        }
    }

    async fn run(
        &mut self,
        sql: &str,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<Ran, tokio_postgres::Error> {
        let statement = self.client.prepare(sql).await?;

        // Statements without a result set run as they are: outside a block
        // in their own implicit transaction (so VACUUM or CREATE DATABASE
        // work), inside one as part of it.
        if statement.columns().is_empty() {
            let affected = self.client.execute(&statement, &[]).await?;
            return Ok(Ran { row_count: affected, cancelled: false, in_transaction: None });
        }

        emit(QueryEvent::Columns {
            columns: statement
                .columns()
                .iter()
                .map(|c| Column { name: c.name().to_owned(), type_name: c.type_().name().to_owned() })
                .collect(),
        });

        let inside = self.transaction_open().await?;
        let (row_count, cancelled) = if inside {
            self.read_in_user_transaction(&statement, sql, page_size, emit).await?
        } else {
            self.read_in_own_transaction(&statement, page_size, emit).await?
        };
        Ok(Ran { row_count, cancelled, in_transaction: Some(inside) })
    }

    /// Pages through a portal inside a transaction of the driver's own; only
    /// called when the user has no transaction block open.
    async fn read_in_own_transaction(
        &mut self,
        statement: &Statement,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<(u64, bool), tokio_postgres::Error> {
        let cancel = self.cancel.clone();
        let read = async {
            let tx = self.client.transaction().await?;
            let portal = tx.bind(statement, &[]).await?;
            let max_rows = i32::try_from(page_size).unwrap_or(i32::MAX);
            let mut total = 0u64;
            loop {
                let rows = tx.query_portal(&portal, max_rows).await?;
                total += rows.len() as u64;
                emit_page(&rows, emit);
                if cancel.requested.load(Ordering::SeqCst) {
                    tx.rollback().await?;
                    return Ok((total, true));
                }
                if rows.len() < page_size {
                    break;
                }
            }
            tx.commit().await?;
            Ok((total, false))
        };
        let outcome = read.await;
        if outcome.is_err() {
            self.end_own_transaction().await;
        }
        outcome
    }

    /// Pages through a cursor declared in a savepoint of the user's
    /// transaction. A cancel between pages only closes the cursor; an error
    /// leaves the transaction aborted, exactly as psql would.
    async fn read_in_user_transaction(
        &mut self,
        statement: &Statement,
        sql: &str,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<(u64, bool), tokio_postgres::Error> {
        self.client.batch_execute("savepoint idedb_read").await?;
        let declare = format!("declare idedb_read no scroll cursor for\n{sql}");
        if self.client.batch_execute(&declare).await.is_err() {
            // Not a statement a cursor can run: undo the attempt so the
            // user's transaction is as it was, and stream it instead.
            self.client.batch_execute("rollback to savepoint idedb_read; release savepoint idedb_read").await?;
            return self.stream_in_user_transaction(statement, page_size, emit).await;
        }

        let fetch = self.client.prepare(&format!("fetch forward {page_size} from idedb_read")).await?;
        let mut total = 0u64;
        let mut cancelled = false;
        loop {
            let rows = self.client.query(&fetch, &[]).await?;
            total += rows.len() as u64;
            emit_page(&rows, emit);
            if self.cancel.requested.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            if rows.len() < page_size {
                break;
            }
        }
        self.client.batch_execute("close idedb_read; release savepoint idedb_read").await?;
        Ok((total, cancelled))
    }

    /// Streams a statement that cannot back a cursor, inside the user's
    /// transaction. Rows arrive as the server sends them, so a cancel drains
    /// what is left; the cancel request itself ends the statement.
    async fn stream_in_user_transaction(
        &mut self,
        statement: &Statement,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<(u64, bool), tokio_postgres::Error> {
        let stream = self.client.query_raw(statement, std::iter::empty::<&(dyn ToSql + Sync)>()).await?;
        pin_mut!(stream);
        let mut page = Vec::with_capacity(page_size.min(4096));
        let mut total = 0u64;
        while let Some(row) = stream.try_next().await? {
            page.push(row);
            if page.len() == page_size {
                total += page.len() as u64;
                emit_page(&std::mem::take(&mut page), emit);
                if self.cancel.requested.load(Ordering::SeqCst) {
                    while stream.try_next().await?.is_some() {}
                    return Ok((total, true));
                }
            }
        }
        total += page.len() as u64;
        emit_page(&page, emit);
        Ok((total, self.cancel.requested.load(Ordering::SeqCst)))
    }

    /// Parse and Describe only: Postgres validates syntax, names and types
    /// when it prepares a statement, and runs nothing until Execute. With a
    /// schema, the search path is set for this check alone (`SET LOCAL`).
    /// Inside the user's transaction block, where a failed Parse would abort
    /// it, the check runs in a savepoint that is rolled back either way.
    async fn prepare_only(&mut self, sql: &str, schema: Option<&str>, inside: bool) -> Result<(), tokio_postgres::Error> {
        let schema = schema.filter(|s| Some(*s) != self.server.default_schema.as_deref());
        if inside {
            self.client.batch_execute("savepoint idedb_check").await?;
            let checked = async {
                if let Some(schema) = schema {
                    self.client.batch_execute(&format!("set local search_path to {}", search_path(schema))).await?;
                }
                self.client.prepare(sql).await.map(drop)
            }
            .await;
            self.client.batch_execute("rollback to savepoint idedb_check; release savepoint idedb_check").await?;
            return checked;
        }
        let Some(schema) = schema else {
            return self.client.prepare(sql).await.map(drop);
        };
        let tx = self.client.transaction().await?;
        tx.batch_execute(&format!("set local search_path to {}", search_path(schema))).await?;
        let prepared = tx.prepare(sql).await.map(drop);
        tx.rollback().await?;
        prepared
    }
}

async fn open(config: &Config, tls: &MakeTlsConnector) -> Result<(Client, JoinHandle<()>), tokio_postgres::Error> {
    let (client, connection) = config.connect(tls.clone()).await?;
    let connection = tokio::spawn(async move {
        // The client observes the closed connection on its next call.
        let _ = connection.await;
    });
    Ok((client, connection))
}

/// Whether the error means the connection is gone: closed, or a FATAL error
/// (the server ends the session right after sending one, e.g. on
/// `pg_terminate_backend` or shutdown).
fn connection_lost(e: &tokio_postgres::Error) -> bool {
    e.is_closed() || e.as_db_error().is_some_and(|db| db.severity() == "FATAL")
}

fn reconnected_notice(lost_transaction: bool, schema: Option<&str>) -> String {
    let restored = schema.map(|s| format!(" (search path `{s}` restored)")).unwrap_or_default();
    let lost = if lost_transaction {
        "The transaction that was open was rolled back by the server: nothing in it was committed."
    } else {
        "Session state such as temporary tables, prepared statements and settings was reset."
    };
    format!("The connection to the server was lost and has been re-established{restored}. {lost}")
}

fn emit_page(rows: &[tokio_postgres::Row], emit: &mut (dyn FnMut(QueryEvent) + Send)) {
    if rows.is_empty() {
        return;
    }
    let page: Vec<Row> = rows
        .iter()
        .map(|row| (0..row.len()).map(|i| row.get::<_, Cell>(i).0).collect())
        .collect();
    emit(QueryEvent::Rows { rows: page });
}

impl Session for PgSession {
    type Canceller = PgCanceller;

    fn canceller(&self) -> PgCanceller {
        PgCanceller { state: self.cancel.clone() }
    }

    fn server_info(&self) -> &ServerInfo {
        &self.server
    }

    async fn execute(&mut self, sql: &str, page_size: usize, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        // A statement never runs on a connection that replaced a lost one
        // without the user hearing about it: what was lost (an open
        // transaction, temporary tables, settings) could change its meaning.
        if self.client.is_closed() {
            let notice = self.recover().await;
            emit(QueryEvent::Error {
                message: format!("{notice} The statement was not run; run it again."),
                position: None,
                in_transaction: false,
            });
            return;
        }

        let cancel = self.cancel.clone();
        cancel.begin();
        let started = Instant::now();
        let outcome = self.run(sql, page_size.max(1), emit).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        cancel.end().await;

        // Reconnect right away so the next statement runs normally; the
        // error tells the user what the lost connection took with it.
        if self.client.is_closed() || outcome.as_ref().is_err_and(connection_lost) {
            let notice = self.recover().await;
            let cause = outcome.err().map(|e| format_error(&e)).unwrap_or_default();
            emit(QueryEvent::Error {
                message: format!("{cause}\n{notice} The statement may or may not have completed."),
                position: None,
                in_transaction: false,
            });
            return;
        }

        // Anything but a read may have opened or closed a transaction block
        // (BEGIN, COMMIT, a failed COMMIT, ...): ask the server.
        let known = outcome.as_ref().ok().and_then(|ran| ran.in_transaction);
        let in_transaction = match known {
            Some(open) => open,
            None => self.transaction_open().await.unwrap_or(self.in_transaction),
        };
        self.in_transaction = in_transaction;

        emit(match outcome {
            Ok(ran) => QueryEvent::Done {
                row_count: ran.row_count,
                elapsed_ms,
                cancelled: ran.cancelled,
                in_transaction,
            },
            Err(e) if e.code() == Some(&SqlState::QUERY_CANCELED) => {
                QueryEvent::Done { row_count: 0, elapsed_ms, cancelled: true, in_transaction }
            }
            Err(e) => QueryEvent::Error { message: format_error(&e), position: error_position(&e), in_transaction },
        });
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        self.ensure_connected().await?;
        introspect::schemas(&self.client).await.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        self.ensure_connected().await?;
        introspect::schema(&self.client, schema).await.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        // Changes meant for a transaction the lost connection took along
        // must not be committed on their own on a fresh one.
        if self.client.is_closed() {
            let notice = self.recover().await;
            return Err(Error::Connect(format!("{notice} Nothing was saved; submit again.")));
        }
        let inside = self.transaction_open().await.map_err(|e| Error::Query(format_error(&e)))?;
        let outcome = apply::apply(&self.client, table, changes, inside).await;
        if outcome.is_err() && !inside {
            self.end_own_transaction().await;
        }
        outcome.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn check(&mut self, sql: &str, schema: Option<&str>) -> Result<Option<SqlProblem>> {
        self.ensure_connected().await?;
        let inside = self.transaction_open().await.map_err(|e| Error::Query(format_error(&e)))?;
        match self.prepare_only(sql, schema, inside).await {
            Ok(()) => Ok(None),
            // Parameters Postgres cannot type without an execution context
            // (`$1` alone in a select list) are not the user's mistake, and
            // an aborted transaction checks nothing.
            Err(e)
                if e.code() == Some(&SqlState::INDETERMINATE_DATATYPE)
                    || e.code() == Some(&SqlState::IN_FAILED_SQL_TRANSACTION) =>
            {
                Ok(None)
            }
            Err(e) if e.as_db_error().is_some() => {
                Ok(Some(SqlProblem { message: format_error(&e), position: error_position(&e) }))
            }
            Err(e) => {
                if !inside {
                    self.end_own_transaction().await;
                }
                Err(Error::Query(format_error(&e)))
            }
        }
    }

    /// Changes the session's `search_path`. Inside the user's transaction
    /// the change is part of it (undone by a ROLLBACK), as a typed `SET`
    /// would be; it never begins or ends a transaction.
    async fn set_schema(&mut self, schema: &str) -> Result<()> {
        self.ensure_connected().await?;
        let default = Some(schema) == self.server.default_schema.as_deref();
        let sql = if default { "reset search_path".to_owned() } else { format!("set search_path to {}", search_path(schema)) };
        self.client.batch_execute(&sql).await.map_err(|e| Error::Query(format_error(&e)))?;
        self.schema = (!default).then(|| schema.to_owned());
        Ok(())
    }
}

/// A search path that starts at `schema` and keeps the server default's
/// entries (`"$user"`, `public`) reachable after it.
fn search_path(schema: &str) -> String {
    let head = quote(schema);
    if schema == "public" { format!("{head}, \"$user\"") } else { format!("{head}, \"$user\", public") }
}

pub(crate) fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

impl Drop for PgSession {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

/// libpq semantics: only `verify-full` checks the certificate chain and host.
fn tls_connector(mode: SslMode) -> Result<MakeTlsConnector> {
    let mut builder = native_tls::TlsConnector::builder();
    if mode != SslMode::VerifyFull {
        builder.danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true);
    }
    let connector = builder.build().map_err(|e| Error::Connect(format!("TLS setup failed: {e}")))?;
    Ok(MakeTlsConnector::new(connector))
}

/// Postgres reports a 1-based character position into the statement.
fn error_position(e: &tokio_postgres::Error) -> Option<u32> {
    match e.as_db_error()?.position()? {
        ErrorPosition::Original(position) => position.checked_sub(1),
        // Positions inside internally generated queries (e.g. a function body)
        // do not point into the user's text.
        ErrorPosition::Internal { .. } => None,
    }
}

/// Server errors in the familiar `psql` shape; client errors with their cause.
fn format_error(e: &tokio_postgres::Error) -> String {
    let Some(db) = e.as_db_error() else {
        return match std::error::Error::source(e) {
            Some(cause) => format!("{e}: {cause}"),
            None => e.to_string(),
        };
    };
    let mut out = format!("{}: {}", db.severity(), db.message());
    if let Some(detail) = db.detail() {
        out.push_str(&format!("\n  Detail: {detail}"));
    }
    if let Some(hint) = db.hint() {
        out.push_str(&format!("\n  Hint: {hint}"));
    }
    out
}
