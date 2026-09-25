//! PostgreSQL sessions built on `tokio-postgres`.
//!
//! Result sets are read through a protocol-level portal inside a
//! transaction, `page_size` rows at a time, so arbitrarily large results
//! never sit fully in memory and any SELECT works without rewriting it into
//! `DECLARE CURSOR`.

mod apply;
mod decode;
mod introspect;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use idedb_core::{
    ApplyOutcome, Canceller, Column, ConnectionParams, Engine, Error, QueryEvent, Result, Row,
    RowChange, SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, SslMode, TableRef,
};
use postgres_native_tls::MakeTlsConnector;
use tokio::task::JoinHandle;
use tokio_postgres::error::{ErrorPosition, SqlState};
use tokio_postgres::{CancelToken, Client, Config};

use crate::decode::Cell;

pub struct PgSession {
    client: Client,
    canceller: PgCanceller,
    server: ServerInfo,
    connection: JoinHandle<()>,
}

/// Cancels the statement currently running on a session. Cloneable so it can
/// be used while the session itself is busy executing.
#[derive(Clone)]
pub struct PgCanceller {
    token: CancelToken,
    tls: MakeTlsConnector,
    requested: Arc<AtomicBool>,
}

impl Canceller for PgCanceller {
    async fn cancel(&self) -> Result<()> {
        // The flag stops paging between fetches, when nothing is running on
        // the server for the cancel request to hit. Repeated cancels are
        // no-ops: every extra request is one more chance to hit a later
        // statement instead.
        if self.requested.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.token
            .cancel_query(self.tls.clone())
            .await
            .map_err(|e| Error::Query(e.to_string()))
    }
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

        let (client, connection) =
            config.connect(tls.clone()).await.map_err(|e| Error::Connect(format_error(&e)))?;
        let connection = tokio::spawn(async move {
            // The client observes the closed connection on its next call.
            let _ = connection.await;
        });

        let row = client
            .query_one("select current_setting('server_version'), current_schema()", &[])
            .await
            .map_err(|e| Error::Connect(format_error(&e)))?;
        let server = ServerInfo {
            engine: Engine::Postgres,
            version: row.get(0),
            default_schema: row.get(1),
        };
        let canceller = PgCanceller {
            token: client.cancel_token(),
            tls,
            requested: Arc::new(AtomicBool::new(false)),
        };
        Ok(Self { client, canceller, server, connection })
    }

    async fn run(
        &mut self,
        sql: &str,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> Result<(u64, bool), tokio_postgres::Error> {
        let statement = self.client.prepare(sql).await?;

        // Statements without a result set run outside an explicit transaction
        // so things like VACUUM or CREATE DATABASE keep working.
        if statement.columns().is_empty() {
            let affected = self.client.execute(&statement, &[]).await?;
            return Ok((affected, false));
        }

        emit(QueryEvent::Columns {
            columns: statement
                .columns()
                .iter()
                .map(|c| Column { name: c.name().to_owned(), type_name: c.type_().name().to_owned() })
                .collect(),
        });

        let requested = self.canceller.requested.clone();
        let tx = self.client.transaction().await?;
        let portal = tx.bind(&statement, &[]).await?;
        let max_rows = i32::try_from(page_size).unwrap_or(i32::MAX);
        let mut total = 0u64;

        loop {
            let rows = tx.query_portal(&portal, max_rows).await?;
            let fetched = rows.len();
            total += fetched as u64;

            let page: Vec<Row> = rows
                .iter()
                .map(|row| (0..row.len()).map(|i| row.get::<_, Cell>(i).0).collect())
                .collect();
            if !page.is_empty() {
                emit(QueryEvent::Rows { rows: page });
            }

            if requested.load(Ordering::SeqCst) {
                tx.rollback().await?;
                return Ok((total, true));
            }
            if fetched < page_size {
                break;
            }
        }
        tx.commit().await?;
        Ok((total, false))
    }
}

impl PgSession {
    /// Makes sure an interrupted read left no transaction open. Cancel
    /// requests are asynchronous and can land on the ROLLBACK that ends the
    /// read, leaving the session in an aborted transaction; outside a
    /// transaction this is a harmless no-op.
    async fn end_transaction(&self) {
        for _ in 0..3 {
            if self.client.batch_execute("rollback").await.is_ok() {
                return;
            }
        }
    }

    /// Parse and Describe only: Postgres validates syntax, names and types
    /// when it prepares a statement, and runs nothing until Execute. With a
    /// schema, the search path is set for this check alone (`SET LOCAL`
    /// inside a transaction that is rolled back).
    async fn prepare_only(&mut self, sql: &str, schema: Option<&str>) -> Result<(), tokio_postgres::Error> {
        let Some(schema) = schema.filter(|s| Some(*s) != self.server.default_schema.as_deref()) else {
            return self.client.prepare(sql).await.map(drop);
        };
        let tx = self.client.transaction().await?;
        tx.batch_execute(&format!("set local search_path to {}", search_path(schema))).await?;
        let prepared = tx.prepare(sql).await.map(drop);
        tx.rollback().await?;
        prepared
    }
}

impl Session for PgSession {
    type Canceller = PgCanceller;

    fn canceller(&self) -> PgCanceller {
        self.canceller.clone()
    }

    fn server_info(&self) -> &ServerInfo {
        &self.server
    }

    async fn execute(&mut self, sql: &str, page_size: usize, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        self.canceller.requested.store(false, Ordering::SeqCst);
        let started = Instant::now();
        let outcome = self.run(sql, page_size.max(1), emit).await;
        if !matches!(outcome, Ok((_, false))) {
            self.end_transaction().await;
        }
        let elapsed_ms = started.elapsed().as_millis() as u64;

        emit(match outcome {
            Ok((row_count, cancelled)) => QueryEvent::Done { row_count, elapsed_ms, cancelled },
            Err(e) if e.code() == Some(&SqlState::QUERY_CANCELED) => {
                QueryEvent::Done { row_count: 0, elapsed_ms, cancelled: true }
            }
            Err(e) => QueryEvent::Error { message: format_error(&e), position: error_position(&e) },
        });
    }

    async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        introspect::schemas(&self.client).await.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        introspect::schema(&self.client, schema).await.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        let outcome = apply::apply(&mut self.client, table, changes).await;
        if !matches!(outcome, Ok(ApplyOutcome::Applied { .. })) {
            self.end_transaction().await;
        }
        outcome.map_err(|e| Error::Query(format_error(&e)))
    }

    async fn check(&mut self, sql: &str, schema: Option<&str>) -> Result<Option<SqlProblem>> {
        match self.prepare_only(sql, schema).await {
            Ok(()) => Ok(None),
            // Parameters Postgres cannot type without an execution context
            // (`$1` alone in a select list) are not the user's mistake.
            Err(e) if e.code() == Some(&SqlState::INDETERMINATE_DATATYPE) => Ok(None),
            Err(e) if e.as_db_error().is_some() => {
                Ok(Some(SqlProblem { message: format_error(&e), position: error_position(&e) }))
            }
            Err(e) => {
                self.end_transaction().await;
                Err(Error::Query(format_error(&e)))
            }
        }
    }

    async fn set_schema(&mut self, schema: &str) -> Result<()> {
        let sql = if Some(schema) == self.server.default_schema.as_deref() {
            "reset search_path".to_owned()
        } else {
            format!("set search_path to {}", search_path(schema))
        };
        self.client.batch_execute(&sql).await.map_err(|e| Error::Query(format_error(&e)))
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
