//! MySQL sessions built on `mysql_async`.
//!
//! Statements run over the text protocol with `query_iter`, which streams
//! rows off the socket as the server sends them, so arbitrarily large
//! results are paged to the UI without ever sitting fully in memory.
//!
//! MySQL has no out-of-band cancel request: cancelling runs
//! `KILL QUERY <id>` over a separate short-lived connection.

mod apply;
mod decode;
mod introspect;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectionParams, Engine, Error, QueryEvent, Result, Row,
    RowChange, SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, SslMode, TableRef,
};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts};
use tokio::sync::Notify;

use crate::decode::Kind;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// `ER_QUERY_INTERRUPTED`: the running statement was hit by `KILL QUERY`.
const QUERY_INTERRUPTED: u16 = 1317;
/// `ER_NO_SUCH_THREAD`: the connection to kill is already gone.
const NO_SUCH_THREAD: u16 = 1094;
/// `ER_UNSUPPORTED_PS`: the statement cannot be prepared, only run.
const UNSUPPORTED_PS: u16 = 1295;

pub struct MySqlSession {
    conn: Conn,
    opts: Opts,
    server: ServerInfo,
    /// Current database: the connect-time one until `set_schema` changes it.
    /// Restored when a broken connection is replaced.
    database: Option<String>,
    cancel: Arc<CancelState>,
    /// Set after a fatal (I/O or protocol) error; the next call reconnects.
    broken: bool,
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
            .prefer_socket(false);

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

    /// Replaces a connection that failed fatally, keeping the selected database.
    async fn reconnect(&mut self) -> Result<(), mysql_async::Error> {
        let opts = OptsBuilder::from_opts(self.opts.clone()).db_name(self.database.clone());
        let conn = Conn::new(opts).await?;
        self.cancel.connection_id.store(conn.id(), Ordering::SeqCst);
        self.conn = conn;
        self.broken = false;
        Ok(())
    }

    async fn run(
        &mut self,
        sql: &str,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<u64, mysql_async::Error> {
        if self.broken {
            self.reconnect().await?;
        }
        let mut result = self.conn.query_iter(sql).await?;

        let Some(columns) = result.columns().filter(|c| !c.is_empty()) else {
            let affected = result.affected_rows();
            result.drop_result().await?;
            return Ok(affected);
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
        let decode = |row: mysql_async::Row| -> Row {
            row.unwrap_raw()
                .into_iter()
                .zip(&kinds)
                .map(|(value, kind)| value.map_or(idedb_core::Value::Null, |v| kind.decode(v)))
                .collect()
        };

        let mut total = 0u64;
        let mut page = Vec::with_capacity(page_size.min(4096));
        loop {
            let Some(row) = result.next().await? else {
                break;
            };
            page.push(decode(row));
            if page.len() == page_size {
                if self.cancel.requested.load(Ordering::SeqCst) {
                    // The KILL already sent ends the stream server-side;
                    // what is still in flight is read and discarded.
                    while let Ok(Some(_)) = result.next().await {}
                    return Ok(total);
                }
                total += page.len() as u64;
                emit(QueryEvent::Rows {
                    rows: std::mem::replace(&mut page, Vec::with_capacity(page_size.min(4096))),
                });
            }
        }
        if !page.is_empty() && !self.cancel.requested.load(Ordering::SeqCst) {
            total += page.len() as u64;
            emit(QueryEvent::Rows { rows: page });
        }
        // Consume any further result sets so the connection is clean.
        result.drop_result().await?;
        Ok(total)
    }
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
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) {
        self.cancel.begin();
        let started = Instant::now();
        let outcome = self.run(sql, page_size.max(1), emit).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        self.cancel.end().await;

        // An interrupted SLEEP() returns normally, so the request flag, not
        // the outcome, decides whether the statement was cancelled.
        let cancelled = self.cancel.requested.load(Ordering::SeqCst);
        emit(match outcome {
            Ok(row_count) => QueryEvent::Done {
                row_count,
                elapsed_ms,
                cancelled,
            },
            Err(e) if cancelled || server_code(&e) == Some(QUERY_INTERRUPTED) => {
                self.broken |= e.is_fatal();
                QueryEvent::Done {
                    row_count: 0,
                    elapsed_ms,
                    cancelled: true,
                }
            }
            Err(e) => {
                self.broken |= e.is_fatal();
                let message = format_error(&e);
                QueryEvent::Error {
                    position: near_position(sql, &message),
                    message,
                }
            }
        });
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        self.ensure_connected().await?;
        introspect::schemas(&mut self.conn)
            .await
            .map_err(|e| self.query_error(e))
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        self.ensure_connected().await?;
        introspect::introspect(&mut self.conn, schema)
            .await
            .map_err(|e| self.query_error(e))
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        self.ensure_connected().await?;
        apply::apply(&mut self.conn, table, changes)
            .await
            .map_err(|e| self.query_error(e))
    }

    /// A server-side prepare (`COM_STMT_PREPARE`), closed right away: MySQL
    /// parses the statement and resolves its tables and columns, and runs
    /// nothing. With a schema, the session switches to that database first
    /// (it only ever serves checks and introspection, which name schemas
    /// explicitly).
    async fn check(&mut self, sql: &str, schema: Option<&str>) -> Result<Option<SqlProblem>> {
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
    async fn ensure_connected(&mut self) -> Result<()> {
        if self.broken {
            self.reconnect()
                .await
                .map_err(|e| Error::Connect(format_error(&e)))?;
        }
        Ok(())
    }

    fn query_error(&mut self, e: mysql_async::Error) -> Error {
        self.broken |= e.is_fatal();
        Error::Query(format_error(&e))
    }
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
