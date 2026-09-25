//! Dispatch over the concrete drivers. `Session` is not object safe (async
//! methods), so the app holds an enum; adding an engine means one variant
//! here. Postgres' session and canceller carry the client and TLS connector
//! inline, so they are boxed to keep the enums small.

use idedb_core::{
    ApplyOutcome, Canceller, ConnectionParams, Engine, QueryEvent, Result, RowChange, SchemaInfo,
    SchemaModel, ServerInfo, Session, SqlProblem, TableRef,
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
        Ok(match params.engine {
            Engine::Postgres => Self::Postgres(Box::new(PgSession::connect(params, password).await?)),
            Engine::Mysql => Self::Mysql(MySqlSession::connect(params, password).await?),
            Engine::Sqlite => Self::Sqlite(SqliteSession::connect(params, password).await?),
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

    pub async fn execute(&mut self, sql: &str, page_size: usize, emit: &mut (dyn FnMut(QueryEvent) + Send)) {
        dispatch!(self, s => s.execute(sql, page_size, emit).await)
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
