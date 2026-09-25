//! Saved data sources: CRUD, passwords and connection tests.

use std::time::Instant;

use idedb_core::{Engine, ServerInfo};
use idedb_store::{DataSource, SecretStore, Store};
use serde::Serialize;
use tauri::State;

use crate::drivers::AnySession;
use crate::error::{CommandError, CommandResult, ErrorCode};
use crate::sessions::Sessions;

pub struct Secrets(pub Box<dyn SecretStore>);

// Commands here are async even without awaiting: Tauri runs sync commands on
// the main thread, and a Keychain access prompt would freeze the UI.

#[tauri::command]
pub async fn data_sources_list(store: State<'_, Store>) -> CommandResult<Vec<DataSource>> {
    Ok(store.list()?)
}

/// `password`: `None` keeps whatever is stored; `Some` replaces it (an
/// empty string stores an empty password).
#[tauri::command]
pub async fn data_source_save(
    source: DataSource,
    password: Option<String>,
    store: State<'_, Store>,
    secrets: State<'_, Secrets>,
) -> CommandResult<DataSource> {
    let saved = store.save(source)?;
    if !saved.save_password {
        secrets.0.delete(&saved.id)?;
    } else if let Some(password) = password {
        secrets.0.set(&saved.id, &password)?;
    }
    Ok(saved)
}

#[tauri::command]
pub async fn data_source_delete(
    id: String,
    store: State<'_, Store>,
    secrets: State<'_, Secrets>,
    sessions: State<'_, Sessions>,
) -> CommandResult<()> {
    sessions.close_data_source(&id).await;
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

/// Connects with unsaved settings. Without a password, falls back to the
/// stored one when the data source already exists.
#[tauri::command]
pub async fn data_source_test(
    source: DataSource,
    password: Option<String>,
    secrets: State<'_, Secrets>,
) -> CommandResult<TestResult> {
    let password = match password {
        Some(p) => Some(p),
        None if !source.id.is_empty() => secrets.0.get(&source.id)?,
        None => None,
    };
    let started = Instant::now();
    let session = AnySession::connect(&source.params, password.as_deref()).await?;
    Ok(TestResult {
        server: session.server_info().clone(),
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// The password to connect with: the one given, else the stored one. When
/// neither exists the UI is told to ask, rather than connecting without
/// one and failing authentication. A passwordless server has an empty
/// password stored.
pub fn resolve_password(
    source: &DataSource,
    given: Option<String>,
    secrets: &dyn SecretStore,
) -> CommandResult<Option<String>> {
    if given.is_some() || source.params.engine == Engine::Sqlite {
        return Ok(given);
    }
    let stored = if source.save_password { secrets.get(&source.id)? } else { None };
    match stored {
        Some(password) => Ok(Some(password)),
        None => Err(CommandError::new(
            ErrorCode::PasswordRequired,
            if source.save_password {
                format!("Password for {} is not in the Keychain", source.name)
            } else {
                format!("Password for {} is not saved", source.name)
            },
        )),
    }
}

#[cfg(test)]
mod tests {
    use idedb_core::{ConnectionParams, SslMode};
    use idedb_store::MemorySecrets;

    use super::*;

    fn source(engine: Engine, save_password: bool) -> DataSource {
        DataSource {
            id: "ds".into(),
            name: "db".into(),
            params: ConnectionParams {
                engine,
                host: "localhost".into(),
                port: None,
                user: "u".into(),
                database: String::new(),
                ssl_mode: SslMode::Prefer,
                path: String::new(),
            },
            color: None,
            save_password,
        }
    }

    fn required(result: CommandResult<Option<String>>) -> bool {
        matches!(result, Err(CommandError { code: ErrorCode::PasswordRequired, .. }))
    }

    #[test]
    fn asks_when_a_saved_password_is_missing_from_the_keychain() {
        let secrets = MemorySecrets::default();
        assert!(required(resolve_password(&source(Engine::Postgres, true), None, &secrets)));
    }

    #[test]
    fn uses_a_stored_password_even_when_empty() {
        let secrets = MemorySecrets::default();
        secrets.set("ds", "").unwrap();
        assert_eq!(resolve_password(&source(Engine::Mysql, true), None, &secrets).unwrap(), Some(String::new()));
        secrets.set("ds", "s3cret").unwrap();
        assert_eq!(resolve_password(&source(Engine::Mysql, true), None, &secrets).unwrap(), Some("s3cret".into()));
    }

    #[test]
    fn asks_for_a_password_that_is_not_saved_and_prefers_the_given_one() {
        let secrets = MemorySecrets::default();
        secrets.set("ds", "stale").unwrap();
        assert!(required(resolve_password(&source(Engine::Postgres, false), None, &secrets)));
        let given = resolve_password(&source(Engine::Postgres, true), Some("typed".into()), &secrets).unwrap();
        assert_eq!(given, Some("typed".into()));
    }

    #[test]
    fn sqlite_never_asks() {
        let secrets = MemorySecrets::default();
        assert_eq!(resolve_password(&source(Engine::Sqlite, true), None, &secrets).unwrap(), None);
    }
}
