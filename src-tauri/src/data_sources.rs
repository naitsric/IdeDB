//! Saved data sources: CRUD, passwords and connection tests.

use std::time::Instant;

use idedb_core::ServerInfo;
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

/// The password to connect with: the one given, else the stored one.
pub fn resolve_password(
    source: &DataSource,
    given: Option<String>,
    secrets: &dyn SecretStore,
) -> CommandResult<Option<String>> {
    if given.is_some() {
        return Ok(given);
    }
    if !source.save_password {
        return Err(CommandError::new(
            ErrorCode::PasswordRequired,
            format!("Password for {} is not saved", source.name),
        ));
    }
    Ok(secrets.get(&source.id)?)
}
