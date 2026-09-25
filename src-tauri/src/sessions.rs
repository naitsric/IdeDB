//! Open database sessions and the commands that drive them.
//!
//! Each console and each data source's explorer gets its own session, so
//! introspection never waits behind a running query. Result sets stream to
//! the UI over a `Channel` as MessagePack-encoded [`QueryEvent`]s; raw bytes
//! skip Tauri's JSON path and arrive in JS as an `ArrayBuffer`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use idedb_core::{QueryEvent, RowChange, SchemaInfo, SchemaModel, ServerInfo, TableRef};
use idedb_store::{HistoryEntry, NewHistoryEntry, Store};
use serde::Serialize;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody, Response};

use crate::data_sources::{Secrets, resolve_password};
use crate::drivers::{AnyCanceller, AnySession};
use crate::error::{CommandError, CommandResult, ErrorCode};

pub type SessionId = u32;

/// Rows per page. Large enough to amortize IPC overhead, small enough that
/// the first page paints almost immediately.
const PAGE_SIZE: usize = 2000;

#[derive(Clone)]
struct Entry {
    data_source_id: String,
    session: Arc<tokio::sync::Mutex<AnySession>>,
    canceller: AnyCanceller,
}

#[derive(Default)]
pub struct Sessions {
    next_id: AtomicU32,
    open: Mutex<HashMap<SessionId, Entry>>,
}

impl Sessions {
    fn get(&self, id: SessionId) -> CommandResult<Entry> {
        self.open
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| CommandError::new(ErrorCode::NotFound, format!("session {id} is not open")))
    }

    /// Closes every session of a data source, e.g. before deleting it.
    pub async fn close_data_source(&self, data_source_id: &str) {
        let closed: Vec<Entry> = {
            let mut open = self.open.lock().unwrap();
            let ids: Vec<SessionId> = open
                .iter()
                .filter(|(_, e)| e.data_source_id == data_source_id)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| open.remove(id)).collect()
        };
        for entry in closed {
            let _ = entry.canceller.cancel().await;
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSession {
    id: SessionId,
    server: ServerInfo,
}

#[tauri::command]
pub async fn session_open(
    data_source_id: String,
    password: Option<String>,
    store: State<'_, Store>,
    secrets: State<'_, Secrets>,
    sessions: State<'_, Sessions>,
) -> CommandResult<OpenedSession> {
    let source = store
        .get(&data_source_id)?
        .ok_or_else(|| CommandError::new(ErrorCode::NotFound, "data source not found"))?;
    let password = resolve_password(&source, password, secrets.0.as_ref())?;
    let session = AnySession::connect(&source.params, password.as_deref()).await?;

    let id = sessions.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let server = session.server_info().clone();
    let canceller = session.canceller();
    sessions.open.lock().unwrap().insert(
        id,
        Entry { data_source_id, session: Arc::new(tokio::sync::Mutex::new(session)), canceller },
    );
    Ok(OpenedSession { id, server })
}

#[tauri::command]
pub async fn session_close(id: SessionId, sessions: State<'_, Sessions>) -> CommandResult<()> {
    let entry = sessions.open.lock().unwrap().remove(&id);
    if let Some(entry) = entry {
        // Stop whatever is running so the session can drop promptly.
        let _ = entry.canceller.cancel().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn session_execute(
    id: SessionId,
    sql: String,
    on_event: Channel<InvokeResponseBody>,
    sessions: State<'_, Sessions>,
    store: State<'_, Store>,
) -> CommandResult<()> {
    let entry = sessions.get(id)?;
    // One statement at a time per session, like a DataGrip console.
    let mut session = entry.session.lock().await;
    let started = Instant::now();
    let mut outcome = None;
    session
        .execute(&sql, PAGE_SIZE, &mut |event| {
            if matches!(event, QueryEvent::Done { .. } | QueryEvent::Error { .. }) {
                outcome = Some(event.clone());
            }
            // A send only fails once the webview is gone; nothing to report to.
            let _ = on_event.send(InvokeResponseBody::Raw(encode(&event)));
        })
        .await;
    drop(session);

    let (elapsed_ms, row_count, error) = match &outcome {
        Some(QueryEvent::Done { row_count, elapsed_ms, cancelled }) => {
            (*elapsed_ms, Some(*row_count), cancelled.then_some("Cancelled"))
        }
        Some(QueryEvent::Error { message, .. }) => (started.elapsed().as_millis() as u64, None, Some(message.as_str())),
        _ => (started.elapsed().as_millis() as u64, None, None),
    };
    // History is a convenience: failing to record it must not fail the run.
    let _ = store.add_history(NewHistoryEntry {
        data_source_id: &entry.data_source_id,
        sql: &sql,
        elapsed_ms: Some(elapsed_ms),
        row_count,
        error,
    });
    Ok(())
}

#[tauri::command]
pub async fn history_list(
    data_source_id: Option<String>,
    search: Option<String>,
    limit: u32,
    store: State<'_, Store>,
) -> CommandResult<Vec<HistoryEntry>> {
    Ok(store.history(data_source_id.as_deref(), search.as_deref(), limit)?)
}

#[tauri::command]
pub async fn session_cancel(id: SessionId, sessions: State<'_, Sessions>) -> CommandResult<()> {
    Ok(sessions.get(id)?.canceller.cancel().await?)
}

#[tauri::command]
pub async fn session_schemas(id: SessionId, sessions: State<'_, Sessions>) -> CommandResult<Vec<SchemaInfo>> {
    let session = sessions.get(id)?.session;
    let mut session = session.lock().await;
    Ok(session.schemas().await?)
}

#[tauri::command]
pub async fn session_introspect(
    id: SessionId,
    schema: String,
    sessions: State<'_, Sessions>,
) -> CommandResult<SchemaModel> {
    let session = sessions.get(id)?.session;
    let mut session = session.lock().await;
    Ok(session.introspect(&schema).await?)
}

/// Applies data editor changes in one transaction. The `ApplyOutcome` goes
/// back as MessagePack, like result rows, so read-back values keep int8
/// precision and bytes stay bytes.
#[tauri::command]
pub async fn session_apply(
    id: SessionId,
    table: TableRef,
    changes: Vec<RowChange>,
    sessions: State<'_, Sessions>,
) -> CommandResult<Response> {
    let session = sessions.get(id)?.session;
    let mut session = session.lock().await;
    let outcome = session.apply(&table, &changes).await?;
    Ok(Response::new(rmp_serde::to_vec_named(&outcome).expect("ApplyOutcome is always serializable")))
}

fn encode(event: &QueryEvent) -> Vec<u8> {
    rmp_serde::to_vec_named(event).expect("QueryEvent is always serializable")
}
