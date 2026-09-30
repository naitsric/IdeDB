//! Running one statement for a tool call: a read on a pooled read-only
//! session inside its engine's guard, or an approved write on a session of
//! its own. Both stop at a row limit and a statement timeout, and release
//! the result (and the locks it holds) as soon as it is read.

use std::sync::Arc;
use std::time::Duration;

use idedb_core::{Column, ConnectOptions, Engine, Fetch, QueryEvent};
use idedb_drivers::{AnyCanceller, AnySession};
use serde_json::Value as Json;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::Host;
use crate::output::Collector;
use crate::pool::{self, Checkout, OpenFailure};

/// How much a statement may read and for how long.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub max_rows: usize,
    pub timeout: Duration,
}

/// What a statement returned.
#[derive(Debug)]
pub(crate) struct Ran {
    /// None for a statement without a result set.
    pub columns: Option<Vec<Column>>,
    pub rows: Vec<Vec<Json>>,
    /// Rows returned, or affected for a statement without a result set.
    pub row_count: u64,
    /// More rows existed than the result holds.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// Why a statement did not return a result.
#[derive(Debug)]
pub(crate) enum Failure {
    /// No session could be opened.
    Open(OpenFailure),
    /// The engine refused or failed the statement, or it was stopped. The
    /// message is for the model.
    Statement { message: String, elapsed_ms: Option<u64> },
}

impl Failure {
    fn statement(message: impl Into<String>) -> Self {
        Self::Statement { message: message.into(), elapsed_ms: None }
    }
}

/// Runs a read on the pooled session, inside its engine's guard:
/// - The schema is set on every call (the one given, else the session's
///   default), before anything else, so no call inherits another's.
/// - Postgres: in a `READ ONLY` transaction with a `SET LOCAL
///   statement_timeout`, rolled back whatever happens. Even a statement that
///   lifts the session's read-only default (`set_config(...)`) is undone,
///   and nothing depends on session state a transaction-pooling proxy loses
///   between transactions.
/// - MySQL: read-only mode is set again, and the server prepares the
///   statement first (running nothing), which refuses a text holding
///   several statements: the text protocol would run them all.
/// - SQLite: the session is read only at the file level, and runs a single
///   statement per call anyway.
///
/// A session left in a state that cannot be trusted (a transaction still
/// open, a guard that failed) is taken out of the pool.
pub(crate) async fn read(
    host: &Arc<dyn Host>,
    checkout: &Checkout,
    sql: &str,
    schema: Option<&str>,
    limits: Limits,
    cancel: &CancellationToken,
) -> Result<Ran, Failure> {
    let opened = checkout.opened();
    let mut pooled = opened.session.lock().await;
    let engine = opened.data_source.params.engine;

    let target = schema.map(str::to_owned).or_else(|| opened.default_schema.clone());
    if target != pooled.schema {
        let set = match &target {
            Some(schema) => pooled.session.set_schema(schema).await.map_err(|e| Failure::statement(e.to_string())),
            // Nothing to set: only a new session has no current schema.
            None => opened.reopen(&mut pooled, host).await.map_err(Failure::Open),
        };
        if let Err(failure) = set {
            checkout.evict();
            return Err(failure);
        }
        pooled.schema = target;
    }

    let session = &mut pooled.session;
    let canceller = opened.canceller();
    let (outcome, reusable) = match engine {
        Engine::Postgres => {
            let begin = simple(session, "begin transaction read only").await;
            let timeout = format!("set local statement_timeout = '{}s'", limits.timeout.as_secs().max(1));
            let guarded = match begin {
                Ok(()) => simple(session, &timeout).await,
                Err(e) => Err(e),
            };
            let outcome = match guarded {
                Ok(()) => statement(session, canceller, sql, limits, cancel).await.0,
                Err(e) => Err(Failure::statement(format!("IdeDB could not start a read-only transaction: {e}"))),
            };
            session.close_result().await;
            // Always, even after a failed BEGIN: whatever the call left open
            // ends here.
            let rolled_back = simple(session, "rollback").await;
            (outcome, rolled_back.is_ok())
        }
        Engine::Mysql => match simple(session, "SET SESSION TRANSACTION READ ONLY").await {
            Err(e) => (Err(Failure::statement(format!("IdeDB could not make the session read only: {e}"))), false),
            Ok(()) => match session.check(sql, None).await {
                Ok(Some(problem)) => (Err(Failure::statement(problem.message)), true),
                Err(e) => (Err(Failure::statement(e.to_string())), true),
                Ok(None) => {
                    let (outcome, in_transaction) = statement(session, canceller, sql, limits, cancel).await;
                    session.close_result().await;
                    (outcome, !in_transaction)
                }
            },
        },
        Engine::Sqlite => {
            let (outcome, in_transaction) = statement(session, canceller, sql, limits, cancel).await;
            session.close_result().await;
            (outcome, !in_transaction)
        }
    };
    if !reusable {
        checkout.evict();
    }
    outcome
}

/// Runs an approved write on a new read-write session of its own, closed
/// right after. Returns at most `limits.max_rows` rows (from `RETURNING`).
/// MySQL prepares it first, like a read, so a text of several statements
/// never runs. A transaction the statement left open is rolled back.
pub(crate) async fn write(
    host: &Arc<dyn Host>,
    data_source_id: &str,
    sql: &str,
    schema: Option<&str>,
    limits: Limits,
    cancel: &CancellationToken,
) -> Result<Ran, Failure> {
    let (source, mut session) =
        pool::connect(host, data_source_id, ConnectOptions::default()).await.map_err(Failure::Open)?;
    if let Some(schema) = schema {
        session.set_schema(schema).await.map_err(|e| Failure::statement(e.to_string()))?;
    }
    if source.params.engine == Engine::Mysql {
        match session.check(sql, None).await {
            Ok(None) => {}
            Ok(Some(problem)) => return Err(Failure::statement(problem.message)),
            Err(e) => return Err(Failure::statement(e.to_string())),
        }
    }
    let canceller = session.canceller();
    let (outcome, in_transaction) = statement(&mut session, canceller, sql, limits, cancel).await;
    session.close_result().await;
    if in_transaction {
        let _ = simple(&mut session, "rollback").await;
        return Err(Failure::statement(
            "The statement left a transaction open, so IdeDB rolled it back: nothing it did was kept.",
        ));
    }
    outcome
}

/// Runs a statement of IdeDB's own, expecting no rows.
async fn simple(session: &mut AnySession, sql: &str) -> Result<(), String> {
    let mut last = None;
    session
        .execute(sql, Fetch::all(1), &mut |event| {
            if matches!(event, QueryEvent::Done { .. } | QueryEvent::Error { .. }) {
                last = Some(event);
            }
        })
        .await;
    match last {
        Some(QueryEvent::Done { cancelled: false, .. }) => Ok(()),
        Some(QueryEvent::Done { .. }) => Err("cancelled".into()),
        Some(QueryEvent::Error { message, .. }) => Err(message),
        _ => Err("the session reported no outcome".into()),
    }
}

/// Why a statement was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Timeout,
    Cancelled,
}

/// Runs the statement itself with `limits`, cancelling it through
/// `canceller` at the timeout or when `cancel` fires. Also returns whether
/// a transaction is open after it.
async fn statement(
    session: &mut AnySession,
    canceller: AnyCanceller,
    sql: &str,
    limits: Limits,
    cancel: &CancellationToken,
) -> (Result<Ran, Failure>, bool) {
    if cancel.is_cancelled() {
        return (Err(Failure::statement(CANCELLED)), false);
    }
    let started = Instant::now();
    let (done, finished) = oneshot::channel::<()>();
    let watcher = tokio::spawn(watch(canceller, limits.timeout, cancel.clone(), finished));
    let mut collector = Collector::new(limits.max_rows);
    let page = limits.max_rows.clamp(1, 1000);
    session.execute(sql, Fetch::first(limits.max_rows, page), &mut |event| collector.push(event)).await;
    drop(done);
    // A cancel request is never left in flight to hit what runs next.
    let stopped = watcher.await.ok().flatten();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let stop_message = |stopped: Option<Stop>| match stopped {
        Some(Stop::Cancelled) => CANCELLED.to_owned(),
        // Postgres' own statement_timeout may beat the timer.
        _ => format!(
            "The statement was cancelled after {}s, IdeDB's statement timeout. Make it cheaper (a narrower WHERE, a \
             LIMIT, an aggregate) or ask the user to raise the timeout in IdeDB's MCP settings.",
            limits.timeout.as_secs()
        ),
    };
    let Collector { columns, rows, dropped, last, .. } = collector;
    match last {
        Some(QueryEvent::Done { cancelled: true, in_transaction, .. }) => {
            (Err(Failure::Statement { message: stop_message(stopped), elapsed_ms: Some(elapsed_ms) }), in_transaction)
        }
        Some(QueryEvent::Done { row_count, has_more, in_transaction, elapsed_ms, .. }) => {
            let row_count = if columns.is_some() { rows.len() as u64 } else { row_count };
            let truncated = has_more || dropped;
            (Ok(Ran { columns, rows, row_count, truncated, elapsed_ms }), in_transaction)
        }
        Some(QueryEvent::Error { message, in_transaction, .. }) => {
            let message = match stopped {
                Some(_) => format!("{} ({message})", stop_message(stopped)),
                None => message,
            };
            (Err(Failure::Statement { message, elapsed_ms: Some(elapsed_ms) }), in_transaction)
        }
        _ => (Err(Failure::statement("The database session reported no outcome for the statement.")), true),
    }
}

const CANCELLED: &str = "The call was cancelled, so IdeDB stopped the statement.";

/// Cancels the statement at `timeout` or when `cancel` fires, unless
/// `finished` resolves first; says which it was. A cancel it started is
/// always seen through, so none is left to hit the next statement.
async fn watch(
    canceller: AnyCanceller,
    timeout: Duration,
    cancel: CancellationToken,
    finished: oneshot::Receiver<()>,
) -> Option<Stop> {
    let stop = tokio::select! {
        // A statement that finished as the timer fired was not stopped.
        biased;
        _ = finished => return None,
        () = tokio::time::sleep(timeout) => Stop::Timeout,
        () = cancel.cancelled() => Stop::Cancelled,
    };
    let _ = canceller.cancel().await;
    Some(stop)
}

/// The engine guards on real servers, through the read path itself (no
/// classifier in front). They read `IDEDB_PG_URL` and `IDEDB_MYSQL_URL` and
/// skip themselves without them.
#[cfg(test)]
mod tests {
    use idedb_core::{ConnectionParams, SslMode};
    use idedb_store::{DataSource, SecretStore};
    use serde_json::json;

    use super::*;
    use crate::pool::Pool;
    use crate::testing::{TestHost, exec};

    struct Server {
        host: Arc<dyn Host>,
        source: DataSource,
        password: String,
        pool: Pool,
    }

    /// A saved data source (password in the secrets) on the server `var`
    /// names, as `scheme://user:password@host:port/database`.
    fn server(var: &str, engine: Engine) -> Option<Server> {
        server_on(var, engine, None)
    }

    fn server_on(var: &str, engine: Engine, database: Option<&str>) -> Option<Server> {
        let Ok(url) = std::env::var(var) else {
            eprintln!("{var} not set, skipping");
            return None;
        };
        let (_, rest) = url.split_once("://").expect("scheme");
        let (auth, address) = rest.rsplit_once('@').expect("credentials");
        let (user, password) = auth.split_once(':').expect("user:password");
        let (host_port, url_database) = address.split_once('/').unwrap_or((address, ""));
        let (host, port) = host_port.split_once(':').expect("host:port");
        let params = ConnectionParams {
            engine,
            host: host.into(),
            port: Some(port.parse().expect("port")),
            user: user.into(),
            database: database.unwrap_or(url_database).into(),
            ssl_mode: SslMode::Disable,
            path: String::new(),
        };
        let test = TestHost::new();
        let source =
            test.save(DataSource { id: String::new(), name: var.into(), params, color: None, save_password: true });
        test.secrets.set(&source.id, password).unwrap();
        Some(Server { host: test, source, password: password.into(), pool: Pool::default() })
    }

    impl Server {
        async fn checkout(&self) -> Checkout {
            self.pool.checkout(&self.host, "client", &self.source).await.map_err(|e| format!("{e:?}")).unwrap()
        }

        async fn exec(&self, sql: &str) -> Vec<idedb_core::Row> {
            exec(&self.source, Some(&self.password), sql).await
        }

        async fn read(&self, checkout: &Checkout, sql: &str, schema: Option<&str>, timeout_secs: u64) -> Result<Ran, Failure> {
            let limits = Limits { max_rows: 10, timeout: Duration::from_secs(timeout_secs) };
            read(&self.host, checkout, sql, schema, limits, &CancellationToken::new()).await
        }

        async fn rows(&self, checkout: &Checkout, sql: &str, schema: Option<&str>) -> Vec<Vec<Json>> {
            match self.read(checkout, sql, schema, 30).await {
                Ok(ran) => ran.rows,
                Err(failure) => panic!("{sql}: {failure:?}"),
            }
        }
    }

    fn statement_error(outcome: Result<Ran, Failure>) -> String {
        match outcome {
            Err(Failure::Statement { message, .. }) => message,
            other => panic!("expected a statement failure, got {other:?}"),
        }
    }

    /// Runs `sql` on the pooled session straight through the driver,
    /// bypassing every guard; returns its final event.
    async fn direct(checkout: &Checkout, sql: &str) -> QueryEvent {
        let mut pooled = checkout.opened().session.lock().await;
        let mut last = None;
        pooled
            .session
            .execute(sql, Fetch::all(10), &mut |event| {
                if matches!(event, QueryEvent::Done { .. } | QueryEvent::Error { .. }) {
                    last = Some(event);
                }
            })
            .await;
        last.expect("a final event")
    }

    fn outside_transaction(event: &QueryEvent) -> bool {
        matches!(event, QueryEvent::Done { in_transaction: false, .. })
    }

    fn probe(name: &str) -> String {
        format!("idedb_mcp_probe_{name}_{}", std::process::id())
    }

    #[tokio::test]
    async fn postgres_rolls_back_whatever_a_read_changed() {
        let Some(server) = server("IDEDB_PG_URL", Engine::Postgres) else { return };
        let probe = probe("rollback");
        server.exec(&format!("create table {probe} (id int)")).await;
        let checkout = server.checkout().await;

        // Lifts the session's read-only default, as far as the statement goes.
        let lift = "select set_config('default_transaction_read_only', 'off', false)";
        assert_eq!(server.rows(&checkout, lift, None).await, [[json!("off")]]);
        // The rollback undid it: the same session still refuses to write.
        let insert = direct(&checkout, &format!("insert into {probe} values (1)")).await;
        assert!(
            matches!(&insert, QueryEvent::Error { message, .. } if message.contains("read-only transaction")),
            "{insert:?}"
        );

        // The timeout is local to the read's transaction.
        let shown = server.read(&checkout, "show statement_timeout", None, 7).await.unwrap();
        assert_eq!(shown.rows, [[json!("7s")]]);
        let after = direct(&checkout, "show statement_timeout").await;
        assert!(outside_transaction(&after), "{after:?}");
        assert_eq!(server.rows(&checkout, "select current_setting('statement_timeout')", None).await, [[json!("30s")]]);

        server.exec(&format!("drop table {probe}")).await;
    }

    #[tokio::test]
    async fn postgres_reads_end_their_transaction_however_they_end() {
        let Some(server) = server("IDEDB_PG_URL", Engine::Postgres) else { return };
        let checkout = server.checkout().await;

        let message = statement_error(server.read(&checkout, "select pg_sleep(5)", None, 1).await);
        assert!(message.starts_with("The statement was cancelled after 1s"), "{message}");
        assert!(outside_transaction(&direct(&checkout, "select 1").await));

        let message = statement_error(server.read(&checkout, "select 1 / 0", None, 30).await);
        assert!(message.contains("division by zero"), "{message}");
        assert!(outside_transaction(&direct(&checkout, "select 1").await));

        // A result past the row limit is closed, not left open.
        let many = server.read(&checkout, "select generate_series(1, 100)", None, 30).await.unwrap();
        assert_eq!((many.rows.len(), many.truncated), (10, true));
        assert!(outside_transaction(&direct(&checkout, "select 1").await));

        // Cancelling the call stops the statement too.
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stop.cancel();
        });
        let limits = Limits { max_rows: 10, timeout: Duration::from_secs(30) };
        let cancelled = read(&server.host, &checkout, "select pg_sleep(5)", None, limits, &cancel).await;
        assert_eq!(statement_error(cancelled), CANCELLED);
        assert!(outside_transaction(&direct(&checkout, "select 1").await));

        // Still pooled: nothing left it untrustworthy.
        assert_eq!(server.pool.len(), 1);
    }

    #[tokio::test]
    async fn postgres_sets_the_schema_on_every_read() {
        let Some(server) = server("IDEDB_PG_URL", Engine::Postgres) else { return };
        let checkout = server.checkout().await;
        let current = "select current_schema()";
        assert_eq!(server.rows(&checkout, current, Some("information_schema")).await, [[json!("information_schema")]]);
        assert_eq!(server.rows(&checkout, current, None).await, [[json!("public")]]);
    }

    #[tokio::test]
    async fn mysql_refuses_several_statements_in_one_read() {
        let Some(server) = server("IDEDB_MYSQL_URL", Engine::Mysql) else { return };
        let probe = probe("multi");
        server.exec(&format!("create table {probe} (id int)")).await;
        let checkout = server.checkout().await;

        let refused = statement_error(server.read(&checkout, &format!("select 1; drop table {probe}"), None, 30).await);
        assert!(refused.contains("ERROR 1064"), "{refused}");
        // Still there.
        assert_eq!(server.exec(&format!("select count(*) from {probe}")).await, [[idedb_core::Value::Int(0)]]);

        // The session is fine for the next read.
        assert_eq!(server.rows(&checkout, "select 1", None).await, [[json!(1)]]);
        server.exec(&format!("drop table {probe}")).await;
    }

    #[tokio::test]
    async fn mysql_reads_stop_at_the_timeout() {
        let Some(server) = server("IDEDB_MYSQL_URL", Engine::Mysql) else { return };
        let checkout = server.checkout().await;
        let message = statement_error(server.read(&checkout, "select sleep(5)", None, 1).await);
        assert!(message.starts_with("The statement was cancelled after 1s"), "{message}");
        assert_eq!(server.rows(&checkout, "select 2", None).await, [[json!(2)]]);
    }

    #[tokio::test]
    async fn mysql_sets_the_schema_on_every_read() {
        // Without a database of its own, going back to none takes a new
        // session: MySQL cannot deselect one.
        let Some(server) = server_on("IDEDB_MYSQL_URL", Engine::Mysql, Some("")) else { return };
        let checkout = server.checkout().await;
        let current = "select database()";
        assert_eq!(server.rows(&checkout, current, None).await, [[Json::Null]]);
        assert_eq!(server.rows(&checkout, current, Some("information_schema")).await, [[json!("information_schema")]]);
        assert_eq!(server.rows(&checkout, current, None).await, [[Json::Null]]);
        assert_eq!(server.rows(&checkout, current, Some("idedb")).await, [[json!("idedb")]]);
    }
}
