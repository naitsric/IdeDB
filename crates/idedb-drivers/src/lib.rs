//! Dispatch over the concrete drivers. `Session` is not object safe (async
//! methods), so the app holds an enum; adding an engine means one variant
//! here. Postgres' session and canceller carry the client and TLS connector
//! inline, so they are boxed to keep the enums small.
//!
//! [`open_data_source`] opens a session on a saved data source.

mod open;

pub use open::{OpenError, open_data_source, resolve_password};

use idedb_core::{
    ApplyOutcome, Canceller, ConnectOptions, ConnectionParams, Engine, Fetch, QueryEvent, Result, RowChange,
    SchemaInfo, SchemaModel, ServerInfo, Session, SqlProblem, TableRef,
};
use idedb_driver_mysql::{MySqlCanceller, MySqlSession};
use idedb_driver_pg::{PgCanceller, PgSession};
use idedb_driver_sqlite::{SqliteCanceller, SqliteSession};

pub enum AnySession {
    Postgres(Box<PgSession>),
    Mysql(MySqlSession),
    Sqlite(SqliteSession),
}

#[derive(Clone)]
pub enum AnyCanceller {
    Postgres(Box<PgCanceller>),
    Mysql(MySqlCanceller),
    Sqlite(SqliteCanceller),
}

macro_rules! dispatch {
    ($value:expr, $inner:ident => $body:expr) => {
        match $value {
            AnySession::Postgres($inner) => $body,
            AnySession::Mysql($inner) => $body,
            AnySession::Sqlite($inner) => $body,
        }
    };
}

impl AnySession {
    pub async fn connect(params: &ConnectionParams, password: Option<&str>) -> Result<Self> {
        Self::connect_with(params, password, ConnectOptions::default()).await
    }

    /// Like [`connect`](Self::connect), set up as `options` asks (see
    /// [`ConnectOptions`]).
    pub async fn connect_with(params: &ConnectionParams, password: Option<&str>, options: ConnectOptions) -> Result<Self> {
        Ok(match params.engine {
            Engine::Postgres => Self::Postgres(Box::new(PgSession::connect_with(params, password, options).await?)),
            Engine::Mysql => Self::Mysql(MySqlSession::connect_with(params, password, options).await?),
            Engine::Sqlite => Self::Sqlite(SqliteSession::connect_with(params, password, options).await?),
        })
    }

    pub fn canceller(&self) -> AnyCanceller {
        match self {
            Self::Postgres(s) => AnyCanceller::Postgres(Box::new(s.canceller())),
            Self::Mysql(s) => AnyCanceller::Mysql(s.canceller()),
            Self::Sqlite(s) => AnyCanceller::Sqlite(s.canceller()),
        }
    }

    pub fn server_info(&self) -> &ServerInfo {
        dispatch!(self, s => s.server_info())
    }

    pub async fn execute(&mut self, sql: &str, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        dispatch!(self, s => s.execute(sql, fetch, emit).await)
    }

    pub async fn fetch_more(&mut self, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        dispatch!(self, s => s.fetch_more(fetch, emit).await)
    }

    pub async fn close_result(&mut self) {
        dispatch!(self, s => s.close_result().await)
    }

    pub async fn schemas(&mut self) -> Result<Vec<SchemaInfo>> {
        dispatch!(self, s => s.schemas().await)
    }

    pub async fn introspect(&mut self, schema: &str) -> Result<SchemaModel> {
        dispatch!(self, s => s.introspect(schema).await)
    }

    pub async fn apply(&mut self, table: &TableRef, changes: &[RowChange]) -> Result<ApplyOutcome> {
        dispatch!(self, s => s.apply(table, changes).await)
    }

    pub async fn check(&mut self, sql: &str, schema: Option<&str>) -> Result<Option<SqlProblem>> {
        dispatch!(self, s => s.check(sql, schema).await)
    }

    pub async fn set_schema(&mut self, schema: &str) -> Result<()> {
        dispatch!(self, s => s.set_schema(schema).await)
    }
}

impl AnyCanceller {
    pub async fn cancel(&self) -> Result<()> {
        match self {
            Self::Postgres(c) => c.cancel().await,
            Self::Mysql(c) => c.cancel().await,
            Self::Sqlite(c) => c.cancel().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use idedb_core::{ConnectOptions, ConnectionParams, Engine, Fetch, QueryEvent, SslMode};

    use super::AnySession;

    fn sqlite(path: &std::path::Path) -> ConnectionParams {
        ConnectionParams {
            engine: Engine::Sqlite,
            host: String::new(),
            port: None,
            user: String::new(),
            database: String::new(),
            ssl_mode: SslMode::Disable,
            path: path.to_str().unwrap().to_owned(),
        }
    }

    async fn last_event(session: &mut AnySession, sql: &str) -> Option<QueryEvent> {
        let mut last = None;
        session.execute(sql, Fetch::all(10), &mut |e| last = Some(e)).await;
        last
    }

    /// The options reach the driver: read only, the session refuses writes.
    #[tokio::test]
    async fn connects_with_options() {
        let dir = tempfile::tempdir().unwrap();
        let params = sqlite(&dir.path().join("test.db"));
        let mut rw = AnySession::connect_with(&params, None, ConnectOptions::default()).await.unwrap();
        assert!(matches!(last_event(&mut rw, "create table t (id int)").await, Some(QueryEvent::Done { .. })));

        let mut ro = AnySession::connect_with(&params, None, ConnectOptions { read_only: true }).await.unwrap();
        let refused = last_event(&mut ro, "insert into t values (1)").await;
        assert!(
            matches!(&refused, Some(QueryEvent::Error { message, .. }) if message.contains("readonly database")),
            "{refused:?}"
        );
    }
}
