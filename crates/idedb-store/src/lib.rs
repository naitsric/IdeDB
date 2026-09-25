//! Saved data sources, their passwords and query history.
//!
//! Data sources live in a local SQLite file; connection parameters are
//! stored as JSON so new fields need no migration. Passwords never touch
//! that file: they go to a [`SecretStore`], the macOS Keychain in the app.

mod history;
mod secrets;

pub use history::{HistoryEntry, NewHistoryEntry};
#[cfg(target_os = "macos")]
pub use secrets::Keychain;
pub use secrets::{MemorySecrets, SecretStore};

use std::path::Path;
use std::sync::Mutex;

use idedb_core::ConnectionParams;
use rusqlite::{Connection, OptionalExtension, params};
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

    fn init(db: Connection) -> Result<Self> {
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            db.execute_batch(migration)?;
            db.pragma_update(None, "user_version", i as i64 + 1)?;
        }
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

    pub fn delete(&self, id: &str) -> Result<()> {
        self.db.lock().unwrap().execute("delete from data_source where id = ?1", [id])?;
        Ok(())
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
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idedb.db");
        let saved = Store::open(&path).unwrap().save(source("persisted")).unwrap();
        assert_eq!(Store::open(&path).unwrap().list().unwrap(), vec![saved]);
    }
}
