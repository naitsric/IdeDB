//! App settings by key. Values are JSON text whose shape the caller owns,
//! e.g. `mcp` holds the MCP server's `{enabled, port, …}`.

use rusqlite::{OptionalExtension, params};

use crate::{Result, Store};

impl Store {
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row("select value from setting where key = ?1", [key], |r| r.get(0)).optional()?)
    }

    /// Inserts or replaces the value.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "insert into setting (key, value) values (?1, ?2)
             on conflict (key) do update set value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_replaces() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.setting("mcp").unwrap(), None);

        store.set_setting("mcp", r#"{"enabled":false,"port":7420}"#).unwrap();
        store.set_setting("other", "1").unwrap();
        assert_eq!(store.setting("mcp").unwrap().as_deref(), Some(r#"{"enabled":false,"port":7420}"#));

        store.set_setting("mcp", r#"{"enabled":true}"#).unwrap();
        assert_eq!(store.setting("mcp").unwrap().as_deref(), Some(r#"{"enabled":true}"#));
        assert_eq!(store.setting("other").unwrap().as_deref(), Some("1"));
    }
}
