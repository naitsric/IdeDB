//! Every statement run from a console, per data source, newest first.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

/// Statements kept per data source; older ones are pruned on insert.
const RETENTION: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub data_source_id: String,
    pub sql: String,
    /// UTC, RFC 3339 with milliseconds, e.g. `2026-09-25T15:04:05.123Z`.
    pub executed_at: String,
    pub elapsed_ms: Option<u64>,
    /// Rows returned, or affected for statements without a result set.
    pub row_count: Option<u64>,
    /// Set when the statement failed.
    pub error: Option<String>,
}

pub struct NewHistoryEntry<'a> {
    pub data_source_id: &'a str,
    pub sql: &'a str,
    pub elapsed_ms: Option<u64>,
    pub row_count: Option<u64>,
    pub error: Option<&'a str>,
}

impl Store {
    /// Records a statement, stamped now.
    pub fn add_history(&self, entry: NewHistoryEntry) -> Result<()> {
        self.add_history_retaining(entry, RETENTION)
    }

    fn add_history_retaining(&self, entry: NewHistoryEntry, keep: usize) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "insert into query_history (data_source_id, sql, executed_at, elapsed_ms, row_count, error)
             values (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3, ?4, ?5)",
            params![
                entry.data_source_id,
                entry.sql,
                entry.elapsed_ms.map(|n| n as i64),
                entry.row_count.map(|n| n as i64),
                entry.error
            ],
        )?;
        // Ids grow with insertion, so everything below the `keep`-th newest
        // id of this data source is older than what is retained.
        db.execute(
            "delete from query_history where data_source_id = ?1 and id < (
               select id from query_history where data_source_id = ?1
               order by id desc limit 1 offset ?2)",
            params![entry.data_source_id, keep as i64 - 1],
        )?;
        Ok(())
    }

    /// Newest first, optionally for one data source and containing `search`
    /// (case-insensitive for ASCII).
    pub fn history(&self, data_source_id: Option<&str>, search: Option<&str>, limit: u32) -> Result<Vec<HistoryEntry>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "select id, data_source_id, sql, executed_at, elapsed_ms, row_count, error
             from query_history
             where (?1 is null or data_source_id = ?1)
               and (?2 is null or instr(lower(sql), lower(?2)) > 0)
             order by executed_at desc, id desc
             limit ?3",
        )?;
        let rows = stmt.query_map(params![data_source_id, search.filter(|s| !s.is_empty()), limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                data_source_id: row.get(1)?,
                sql: row.get(2)?,
                executed_at: row.get(3)?,
                elapsed_ms: row.get::<_, Option<i64>>(4)?.map(|n| n as u64),
                row_count: row.get::<_, Option<i64>>(5)?.map(|n| n as u64),
                error: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(source: &'a str, sql: &'a str) -> NewHistoryEntry<'a> {
        NewHistoryEntry { data_source_id: source, sql, elapsed_ms: Some(3), row_count: Some(1), error: None }
    }

    fn sqls(entries: &[HistoryEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.sql.as_str()).collect()
    }

    #[test]
    fn lists_newest_first_per_source() {
        let store = Store::in_memory().unwrap();
        store.add_history(entry("a", "select 1")).unwrap();
        store.add_history(entry("b", "select 2")).unwrap();
        store
            .add_history(NewHistoryEntry { error: Some("boom"), row_count: None, ..entry("a", "select 3") })
            .unwrap();

        let a = store.history(Some("a"), None, 10).unwrap();
        assert_eq!(sqls(&a), ["select 3", "select 1"]);
        assert_eq!(a[0].error.as_deref(), Some("boom"));
        assert_eq!(a[0].row_count, None);
        assert_eq!(a[1].elapsed_ms, Some(3));
        assert!(a[0].executed_at.ends_with('Z') && a[0].executed_at.contains('T'), "{}", a[0].executed_at);

        assert_eq!(sqls(&store.history(None, None, 10).unwrap()), ["select 3", "select 2", "select 1"]);
        assert_eq!(sqls(&store.history(None, None, 1).unwrap()), ["select 3"]);
    }

    #[test]
    fn filters_by_case_insensitive_substring() {
        let store = Store::in_memory().unwrap();
        store.add_history(entry("a", "SELECT * FROM Orders")).unwrap();
        store.add_history(entry("a", "select * from customers")).unwrap();

        assert_eq!(sqls(&store.history(Some("a"), Some("orders"), 10).unwrap()), ["SELECT * FROM Orders"]);
        assert_eq!(store.history(Some("a"), Some(""), 10).unwrap().len(), 2);
        assert!(store.history(Some("a"), Some("missing"), 10).unwrap().is_empty());
    }

    #[test]
    fn prunes_to_the_newest_per_source() {
        let store = Store::in_memory().unwrap();
        for i in 0..5 {
            store.add_history_retaining(entry("a", &format!("select {i}")), 3).unwrap();
        }
        store.add_history_retaining(entry("b", "select b"), 3).unwrap();

        assert_eq!(sqls(&store.history(Some("a"), None, 10).unwrap()), ["select 4", "select 3", "select 2"]);
        assert_eq!(sqls(&store.history(Some("b"), None, 10).unwrap()), ["select b"]);
    }

    #[test]
    fn migrates_an_existing_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("idedb.db");
        {
            // A store created before history existed.
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(crate::MIGRATIONS[0]).unwrap();
            db.pragma_update(None, "user_version", 1).unwrap();
        }
        let store = Store::open(&path).unwrap();
        store.add_history(entry("a", "select 1")).unwrap();
        assert_eq!(store.history(None, None, 10).unwrap().len(), 1);
    }
}
