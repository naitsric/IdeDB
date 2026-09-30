//! Saved data sources, their passwords, query history, app settings and the
//! MCP server's clients, grants and audit log.
//!
//! Data sources live in a local SQLite file; connection parameters are
//! stored as JSON so new fields need no migration. Passwords never touch
//! that file: they go to a [`SecretStore`], the macOS Keychain in the app.
//!
//! Foreign keys are not enforced: deleting a row cleans up what refers to it
//! explicitly (see [`Store::delete`]).

mod history;
mod mcp;
mod secrets;
mod settings;

pub use history::{HistoryEntry, NewHistoryEntry};
pub use mcp::{Access, AuditEntry, AuditFilter, Decision, Grant, McpClient, NewAuditEntry, Transport};
#[cfg(target_os = "macos")]
pub use secrets::Keychain;
pub use secrets::{MemorySecrets, SecretStore};

use std::path::Path;
use std::sync::Mutex;

use idedb_core::ConnectionParams;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("corrupt data source {id}: {source}")]
    Corrupt { id: String, source: serde_json::Error },
    #[error("keychain error: {0}")]
    Secret(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSource {
    /// Empty for a data source not saved yet; [`Store::save`] assigns one.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub params: ConnectionParams,
    /// Accent shown in the explorer and console tabs, e.g. `#e5484d` for production.
    pub color: Option<String>,
    /// When false the password is asked for on every connect and never stored.
    pub save_password: bool,
}

/// Applied in order; `PRAGMA user_version` records how many ran. Only ever append.
const MIGRATIONS: &[&str] = &[
    "create table data_source (
        id text primary key,
        name text not null,
        params text not null,
        color text,
        save_password integer not null,
        position integer not null
    )",
    "create table query_history (
        id integer primary key,
        data_source_id text not null,
        sql text not null,
        executed_at text not null,
        elapsed_ms integer,
        row_count integer,
        error text
    );
    create index query_history_by_source on query_history (data_source_id, executed_at)",
    // Generic app settings (JSON values), then the MCP server: clients with
    // their token hash, per data source grants and policy, and the audit log.
    // Audit rows copy client and data source names so they outlive both.
    "create table setting (
        key text primary key,
        value text not null
    );
    create table mcp_client (
        id text primary key,
        name text not null,
        token_hash blob not null unique,
        token_prefix text not null,
        created_at text not null,
        last_seen_at text,
        last_client_name text,
        last_client_version text,
        revoked_at text
    );
    create table mcp_grant (
        client_id text not null,
        data_source_id text not null,
        access text not null check (access in ('read', 'write')),
        primary key (client_id, data_source_id)
    );
    create table mcp_data_source (
        data_source_id text primary key,
        never_write integer not null default 0
    );
    create table mcp_audit (
        id integer primary key,
        at text not null,
        client_id text,
        client_name text not null,
        client_info_name text,
        client_info_version text,
        protocol_version text,
        transport text not null check (transport in ('http', 'bridge')),
        session_key text,
        tool text not null,
        data_source_id text,
        data_source_name text,
        sql text,
        sql_truncated integer not null default 0,
        statement_kind text,
        reason text,
        decision text not null
            check (decision in ('allowed', 'approved', 'rejected', 'denied', 'timeout', 'withdrawn')),
        approval_wait_ms integer,
        elapsed_ms integer,
        row_count integer,
        truncated integer not null default 0,
        error text
    );
    create index mcp_audit_by_client on mcp_audit (client_id, id);
    create index mcp_audit_by_source on mcp_audit (data_source_id, id)",
];

pub struct Store {
    db: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut db: Connection) -> Result<Self> {
        migrate(&mut db, MIGRATIONS)?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// In the order the user arranged them.
    pub fn list(&self) -> Result<Vec<DataSource>> {
        let db = self.db.lock().unwrap();
        let mut stmt =
            db.prepare("select id, name, params, color, save_password from data_source order by position")?;
        let rows = stmt.query_map([], read_row)?;
        rows.map(|r| r?).collect()
    }

    pub fn get(&self, id: &str) -> Result<Option<DataSource>> {
        let db = self.db.lock().unwrap();
        db.query_row(
            "select id, name, params, color, save_password from data_source where id = ?1",
            [id],
            read_row,
        )
        .optional()?
        .transpose()
    }

    /// Inserts (assigning an id, appended last) or updates in place.
    pub fn save(&self, mut source: DataSource) -> Result<DataSource> {
        if source.id.is_empty() {
            source.id = uuid::Uuid::new_v4().to_string();
        }
        let params_json = serde_json::to_string(&source.params).expect("params serialize");
        let db = self.db.lock().unwrap();
        db.execute(
            "insert into data_source (id, name, params, color, save_password, position)
             values (?1, ?2, ?3, ?4, ?5, (select coalesce(max(position), 0) + 1 from data_source))
             on conflict (id) do update set
               name = excluded.name, params = excluded.params,
               color = excluded.color, save_password = excluded.save_password",
            params![source.id, source.name, params_json, source.color, source.save_password],
        )?;
        Ok(source)
    }

    /// Also drops the data source's MCP grants and policy, in the same
    /// transaction. Its query history and MCP audit rows stay; audit rows
    /// keep the data source's name.
    pub fn delete(&self, id: &str) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("delete from data_source where id = ?1", [id])?;
        mcp::forget_data_source(&tx, id)?;
        tx.commit()?;
        Ok(())
    }
}

/// Runs the migrations past `user_version`, each one and its version bump in
/// one transaction, so a failing migration leaves the store as it was.
///
/// Another process may be opening the same store: each transaction takes
/// the write lock up front (waiting out the other one, up to rusqlite's
/// busy timeout) and re-reads the version under it, so no migration runs
/// twice.
fn migrate(db: &mut Connection, migrations: &[&str]) -> Result<()> {
    let user_version = |db: &Connection| db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0));
    // Up to date, the common case: no need for the write lock.
    if user_version(db)? as usize >= migrations.len() {
        return Ok(());
    }
    loop {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version = user_version(&tx)?;
        let Some(migration) = migrations.get(version as usize) else { return Ok(()) };
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

fn read_row(row: &rusqlite::Row) -> rusqlite::Result<Result<DataSource>> {
    let id: String = row.get(0)?;
    let params_json: String = row.get(2)?;
    let params = match serde_json::from_str(&params_json) {
        Ok(params) => params,
        Err(source) => return Ok(Err(Error::Corrupt { id, source })),
    };
    Ok(Ok(DataSource {
        id,
        name: row.get(1)?,
        params,
        color: row.get(3)?,
        save_password: row.get(4)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use idedb_core::{Engine, SslMode};

    fn source(name: &str) -> DataSource {
        DataSource {
            id: String::new(),
            name: name.into(),
            params: ConnectionParams {
                engine: Engine::Postgres,
                host: "localhost".into(),
                port: Some(54329),
                user: "idedb".into(),
                database: "idedb".into(),
                ssl_mode: SslMode::Prefer,
                path: String::new(),
            },
            color: None,
            save_password: true,
        }
    }

    #[test]
    fn saves_lists_updates_and_deletes_in_order() {
        let store = Store::in_memory().unwrap();
        let a = store.save(source("a")).unwrap();
        let b = store.save(source("b")).unwrap();
        assert!(!a.id.is_empty() && a.id != b.id);

        let renamed = store.save(DataSource { name: "a2".into(), color: Some("#e5484d".into()), ..a.clone() }).unwrap();
        assert_eq!(store.list().unwrap(), vec![renamed.clone(), b.clone()]);
        assert_eq!(store.get(&a.id).unwrap(), Some(renamed));

        store.delete(&a.id).unwrap();
        assert_eq!(store.list().unwrap(), vec![b]);
        assert_eq!(store.get(&a.id).unwrap(), None);
    }

    #[test]
    fn concurrent_opens_migrate_once() {
        // Several processes (here threads, each with its own connection)
        // opening a new store at once: each migration must run exactly once.
        for _ in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("idedb.db");
            let start = std::sync::Barrier::new(4);
            std::thread::scope(|s| {
                let opens: Vec<_> = (0..4)
                    .map(|_| {
                        s.spawn(|| {
                            start.wait();
                            Store::open(&path).map(drop)
                        })
                    })
                    .collect();
                for open in opens {
                    open.join().unwrap().expect("open");
                }
            });
            let store = Store::open(&path).unwrap();
            let version: i64 = store.db.lock().unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
            assert_eq!(version, MIGRATIONS.len() as i64);
        }
    }

    #[test]
    fn a_failing_migration_changes_nothing() {
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db, &["create table a (x)"]).unwrap();
        // The second migration creates `b`, then fails on a syntax error.
        assert!(migrate(&mut db, &["create table a (x)", "create table b (x); create table c ("]).is_err());

        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 1);
        let tables: Vec<String> = db
            .prepare("select name from sqlite_master where type = 'table' order by name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(tables, ["a"]);
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idedb.db");
        let saved = Store::open(&path).unwrap().save(source("persisted")).unwrap();
        assert_eq!(Store::open(&path).unwrap().list().unwrap(), vec![saved]);
    }
}
