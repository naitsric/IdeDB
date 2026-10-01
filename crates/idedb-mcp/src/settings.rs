//! The server's settings, stored as JSON under the store setting `mcp`.

use std::time::Duration;

use idedb_store::Store;
use serde::{Deserialize, Serialize};

use crate::Error;

/// Most rows a client may ask `query` for.
pub const MAX_ROWS: u32 = 1000;

/// The port the server listens on unless the user picks another.
pub const DEFAULT_PORT: u16 = 7412;

/// Longest statement timeout and approval wait the settings accept.
const MAX_SECS: u64 = 3600;
/// Longest write timeout: approved schema changes on big tables take long.
const MAX_WRITE_SECS: u64 = 24 * 3600;

/// The longest a tool call can take: the longest wait for approval, then
/// the longest approved write.
pub(crate) const LONGEST_CALL: Duration = Duration::from_secs(MAX_SECS + MAX_WRITE_SECS);

const KEY: &str = "mcp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct McpSettings {
    /// Whether the server listens. Off until the user turns it on.
    pub enabled: bool,
    /// The loopback port it listens on.
    pub port: u16,
    /// Rows a read returns unless the client asks for another number (up to
    /// [`MAX_ROWS`]), and rows an approved write returns (`RETURNING`).
    pub max_rows: u32,
    /// Reads still running after this long are cancelled.
    pub statement_timeout_secs: u64,
    /// Approved writes still running after this long are cancelled.
    pub write_timeout_secs: u64,
    /// A write nobody approves or rejects within this long is refused.
    pub approval_timeout_secs: u64,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            max_rows: 200,
            statement_timeout_secs: 30,
            write_timeout_secs: 600,
            approval_timeout_secs: 120,
        }
    }
}

impl McpSettings {
    /// What is wrong with the settings, for the settings form.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("the port must be between 1 and 65535".into());
        }
        if !(1..=MAX_ROWS).contains(&self.max_rows) {
            return Err(format!("the row limit must be between 1 and {MAX_ROWS}"));
        }
        if !(1..=MAX_SECS).contains(&self.statement_timeout_secs) {
            return Err(format!("the statement timeout must be between 1 and {MAX_SECS} seconds"));
        }
        if !(1..=MAX_WRITE_SECS).contains(&self.write_timeout_secs) {
            return Err(format!("the write timeout must be between 1 and {MAX_WRITE_SECS} seconds"));
        }
        if !(1..=MAX_SECS).contains(&self.approval_timeout_secs) {
            return Err(format!("the approval timeout must be between 1 and {MAX_SECS} seconds"));
        }
        Ok(())
    }

    pub fn statement_timeout(&self) -> Duration {
        Duration::from_secs(self.statement_timeout_secs)
    }

    pub fn write_timeout(&self) -> Duration {
        Duration::from_secs(self.write_timeout_secs)
    }

    pub fn approval_timeout(&self) -> Duration {
        Duration::from_secs(self.approval_timeout_secs)
    }

    /// Brings stored values that `validate` would refuse into range.
    fn clamped(self) -> Self {
        Self {
            max_rows: self.max_rows.clamp(1, MAX_ROWS),
            statement_timeout_secs: self.statement_timeout_secs.clamp(1, MAX_SECS),
            write_timeout_secs: self.write_timeout_secs.clamp(1, MAX_WRITE_SECS),
            approval_timeout_secs: self.approval_timeout_secs.clamp(1, MAX_SECS),
            ..self
        }
    }
}

/// Defaults when nothing is stored. A value that no longer parses also
/// reads as the defaults, which leave the server off.
pub(crate) fn load(store: &Store) -> Result<McpSettings, idedb_store::Error> {
    let stored = store.setting(KEY)?;
    let settings = stored.and_then(|json| serde_json::from_str::<McpSettings>(&json).ok()).unwrap_or_default();
    Ok(settings.clamped())
}

pub(crate) fn save(store: &Store, settings: &McpSettings) -> Result<(), Error> {
    settings.validate().map_err(Error::InvalidSettings)?;
    let json = serde_json::to_string(settings).expect("settings serialize");
    Ok(store.set_setting(KEY, &json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_absent_partial_or_corrupt() {
        let store = Store::in_memory().unwrap();
        assert_eq!(load(&store).unwrap(), McpSettings::default());
        store.set_setting(KEY, r#"{"enabled":true,"maxRows":50}"#).unwrap();
        assert_eq!(load(&store).unwrap(), McpSettings { enabled: true, max_rows: 50, ..McpSettings::default() });
        store.set_setting(KEY, "not json").unwrap();
        assert_eq!(load(&store).unwrap(), McpSettings::default());
        // Out of range values read back in range.
        store.set_setting(KEY, r#"{"maxRows":5000,"statementTimeoutSecs":0,"writeTimeoutSecs":999999}"#).unwrap();
        let clamped = load(&store).unwrap();
        assert_eq!((clamped.max_rows, clamped.statement_timeout_secs), (MAX_ROWS, 1));
        assert_eq!(clamped.write_timeout_secs, MAX_WRITE_SECS);
        // Settings stored before the write timeout existed get its default.
        store.set_setting(KEY, r#"{"statementTimeoutSecs":5}"#).unwrap();
        assert_eq!(load(&store).unwrap().write_timeout(), Duration::from_secs(600));
    }

    #[test]
    fn saves_valid_settings_in_camel_case() {
        let store = Store::in_memory().unwrap();
        let settings = McpSettings { enabled: true, port: 7500, ..McpSettings::default() };
        save(&store, &settings).unwrap();
        assert_eq!(load(&store).unwrap(), settings);
        let json: serde_json::Value = serde_json::from_str(&store.setting(KEY).unwrap().unwrap()).unwrap();
        assert_eq!(json["statementTimeoutSecs"], 30);
        assert_eq!(json["writeTimeoutSecs"], 600);
        assert_eq!(json["approvalTimeoutSecs"], 120);

        for invalid in [
            McpSettings { max_rows: 0, ..McpSettings::default() },
            McpSettings { write_timeout_secs: 0, ..McpSettings::default() },
            McpSettings { write_timeout_secs: MAX_WRITE_SECS + 1, ..McpSettings::default() },
        ] {
            assert!(matches!(save(&store, &invalid), Err(Error::InvalidSettings(_))), "{invalid:?}");
        }
        assert_eq!(load(&store).unwrap(), settings);
    }
}
