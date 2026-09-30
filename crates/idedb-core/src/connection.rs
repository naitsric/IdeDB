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
    /// The engine itself refuses writes, whatever the SQL: Postgres and
    /// MySQL make every transaction of the session read only (reconnecting
    /// restores it), SQLite opens the file read only and cannot attach
    /// others. Only SQLite still lets the session create and write
    /// temporary tables, which live outside the file.
    ///
    /// In Postgres and MySQL a statement can lift it (`SET SESSION
    /// CHARACTERISTICS AS TRANSACTION READ WRITE` or `BEGIN READ WRITE`;
    /// `SET SESSION TRANSACTION READ WRITE` or `START TRANSACTION READ
    /// WRITE`), so whoever runs untrusted SQL on such a session must also
    /// refuse those. Only a read-only database user closes that for good.
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
