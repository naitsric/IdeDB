//! Saved data sources: CRUD, passwords and connection tests.

use std::time::Instant;

use idedb_core::ServerInfo;
use idedb_drivers::{AnySession, OpenError, resolve_password};
use idedb_store::{DataSource, SecretStore, Store};
use serde::Serialize;
use tauri::State;

use crate::error::{CommandError, CommandResult};
use crate::mcp::McpState;
use crate::sessions::Sessions;

pub struct Secrets(pub Box<dyn SecretStore>);

// Commands here are async even without awaiting: Tauri runs sync commands on
// the main thread, and a Keychain access prompt would freeze the UI.

#[tauri::command]
pub async fn data_sources_list(store: State<'_, Store>) -> CommandResult<Vec<DataSource>> {
    Ok(store.list()?)
}

/// `password`: `None` keeps whatever is stored; `Some` replaces it (an
/// empty string stores an empty password). The MCP server's sessions on the
/// data source close, so its next calls connect with what was saved.
#[tauri::command]
pub async fn data_source_save(
    source: DataSource,
    password: Option<String>,
    store: State<'_, Store>,
    secrets: State<'_, Secrets>,
    mcp: State<'_, McpState>,
) -> CommandResult<DataSource> {
    let saved = store.save(source)?;
    if !saved.save_password {
        secrets.0.delete(&saved.id)?;
    } else if let Some(password) = password {
        secrets.0.set(&saved.id, &password)?;
    }
    mcp.0.close_data_source(&saved.id).await;
    Ok(saved)
}

#[tauri::command]
pub async fn data_source_delete(
    id: String,
    store: State<'_, Store>,
    secrets: State<'_, Secrets>,
    sessions: State<'_, Sessions>,
    mcp: State<'_, McpState>,
) -> CommandResult<()> {
    sessions.close_data_source(&id).await;
    mcp.0.close_data_source(&id).await;
    store.delete(&id)?;
    secrets.0.delete(&id)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    server: ServerInfo,
    latency_ms: u64,
}

/// Connects with unsaved settings, with the password typed in the dialog or
/// else the one connecting would use (see [`test_password`]).
#[tauri::command]
pub async fn data_source_test(
    source: DataSource,
    password: Option<String>,
    secrets: State<'_, Secrets>,
) -> CommandResult<TestResult> {
    let password = test_password(&source, password, secrets.0.as_ref())?;
    let started = Instant::now();
    let session = AnySession::connect(&source.params, password.as_deref()).await?;
    Ok(TestResult {
        server: session.server_info().clone(),
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// The password a connection test uses: the one typed, else the stored one
/// as connecting would pick it ([`resolve_password`]): only when the edited
/// settings save it, never for SQLite, and never for a data source not
/// saved yet. Where connecting would ask for one, the test tries without,
/// so a server that needs none passes and any other reports the
/// authentication failure.
fn test_password(
    source: &DataSource,
    typed: Option<String>,
    secrets: &dyn SecretStore,
) -> CommandResult<Option<String>> {
    if typed.is_none() && source.id.is_empty() {
        return Ok(None);
    }
    match resolve_password(source, typed, secrets) {
        Ok(password) => Ok(password),
        Err(OpenError::PasswordRequired(_)) => Ok(None),
        Err(e) => Err(CommandError::from(e)),
    }
}

#[cfg(test)]
mod tests {
    use idedb_core::{ConnectionParams, Engine, SslMode};
    use idedb_store::MemorySecrets;

    use super::*;

    fn source(id: &str, engine: Engine, save_password: bool) -> DataSource {
        DataSource {
            id: id.into(),
            name: "db".into(),
            params: ConnectionParams {
                engine,
                host: "localhost".into(),
                port: None,
                user: "u".into(),
                database: String::new(),
                ssl_mode: SslMode::Prefer,
                path: "test.db".into(),
            },
            color: None,
            save_password,
        }
    }

    /// Fails the test if the Keychain is read at all.
    struct Untouchable;

    impl SecretStore for Untouchable {
        fn get(&self, id: &str) -> idedb_store::Result<Option<String>> {
            panic!("read the password of {id:?}")
        }
        fn set(&self, _: &str, _: &str) -> idedb_store::Result<()> {
            unreachable!()
        }
        fn delete(&self, _: &str) -> idedb_store::Result<()> {
            unreachable!()
        }
    }

    fn stored(password: &str) -> MemorySecrets {
        let secrets = MemorySecrets::default();
        secrets.set("ds", password).unwrap();
        secrets
    }

    #[test]
    fn a_typed_password_wins() {
        let typed = test_password(&source("ds", Engine::Postgres, true), Some("typed".into()), &stored("old"));
        assert_eq!(typed.unwrap().as_deref(), Some("typed"));
    }

    #[test]
    fn falls_back_to_the_stored_password_only_while_it_is_saved() {
        let secrets = stored("s3cret");
        let saved = test_password(&source("ds", Engine::Mysql, true), None, &secrets).unwrap();
        assert_eq!(saved.as_deref(), Some("s3cret"));
        // Unticking "Save in Keychain" drops the stored password on save.
        assert_eq!(test_password(&source("ds", Engine::Mysql, false), None, &secrets).unwrap(), None);
    }

    #[test]
    fn tries_without_a_password_where_connecting_would_ask() {
        let missing = test_password(&source("ds", Engine::Postgres, true), None, &MemorySecrets::default());
        assert_eq!(missing.unwrap(), None);
    }

    #[test]
    fn never_reads_the_keychain_for_sqlite_or_a_new_data_source() {
        assert_eq!(test_password(&source("ds", Engine::Sqlite, true), None, &Untouchable).unwrap(), None);
        assert_eq!(test_password(&source("", Engine::Postgres, true), None, &Untouchable).unwrap(), None);
    }
}
