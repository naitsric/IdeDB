use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Mysql,
    Sqlite,
}

impl Engine {
    pub fn default_port(self) -> Option<u16> {
        match self {
            Engine::Postgres => Some(5432),
            Engine::Mysql => Some(3306),
            Engine::Sqlite => None,
        }
    }
}

/// TLS policy, with libpq semantics: `Prefer` and `Require` encrypt without
/// verifying the certificate; only `VerifyFull` checks chain and hostname.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyFull,
}

/// Everything needed to connect except the password, which lives in the OS
/// keychain and is passed separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionParams {
    pub engine: Engine,
    #[serde(default)]
    pub host: String,
    /// `None` means the engine's default port.
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
    /// Database file, for SQLite.
    #[serde(default)]
    pub path: String,
}

impl ConnectionParams {
    pub fn port_or_default(&self) -> u16 {
        self.port.or(self.engine.default_port()).unwrap_or(0)
    }
}

/// How a driver's `connect_with` sets up the session, beyond where to
/// connect. The default is what `connect` does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Makes read only the session's default, which the engine then enforces
    /// on statements that leave that default alone.
    /// - Postgres and MySQL start every transaction of the session read only.
    ///   Connecting and reconnecting fail if the server does not report the
    ///   setting as applied.
    /// - SQLite opens the file read only and cannot attach other files. It
    ///   still lets the session create and write temporary tables, which
    ///   live outside the file.
    ///
    /// It is a default, not a sandbox. In Postgres and MySQL a single
    /// statement can lift it for the rest of the session:
    /// - Postgres: `SET default_transaction_read_only = off`, or
    ///   `set_config('default_transaction_read_only', 'off', false)`. The
    ///   latter works even in a read, and sticks because the driver's own
    ///   read transaction commits. Also `RESET ALL`, `DISCARD ALL`, `SET
    ///   SESSION CHARACTERISTICS AS TRANSACTION READ WRITE`, `BEGIN READ
    ///   WRITE`, `SET TRANSACTION READ WRITE` after `BEGIN`, and any of these
    ///   in a `DO` block.
    /// - MySQL: `SET SESSION transaction_read_only = 0`, `SET
    ///   @@transaction_read_only = 0`, `SET TRANSACTION READ WRITE`, `START
    ///   TRANSACTION READ WRITE`. And one `execute` call runs every statement
    ///   of a multi-statement text: in `SET ...; DELETE ...` both run.
    ///
    /// Nor does it stop side effects the database user is allowed to cause
    /// without writing a table:
    /// - Postgres: `COPY ... TO PROGRAM` or to a file, `ALTER SYSTEM` with
    ///   `pg_reload_conf()`, `lo_export`, `pg_terminate_backend`, `dblink`
    ///   and foreign data wrappers.
    /// - MySQL: `SELECT ... INTO OUTFILE`, `SET GLOBAL` or `SET PERSIST`,
    ///   `KILL`.
    ///
    /// Whoever runs untrusted SQL on such a session must also classify every
    /// statement, guard every call, and connect as a database user that can
    /// only read.
    ///
    /// In Postgres the setting belongs to the server backend, not to the
    /// client. Behind a transaction-pooling pooler (pgbouncer, Supavisor) it
    /// does not follow the client to the backend that runs its next
    /// transaction. It can even stay behind and make other clients'
    /// transactions read only.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub engine: Engine,
    /// Human-readable server version, e.g. `17.2` or `8.4.3`.
    pub version: String,
    /// Schema (Postgres), database (MySQL) or `main` (SQLite) that unqualified names resolve to.
    pub default_schema: Option<String>,
}
