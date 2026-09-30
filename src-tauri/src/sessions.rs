//! Open database sessions and the commands that drive them.
//!
//! Each console and each data source's explorer gets its own session, so
//! introspection never waits behind a running query. Result sets stream to
//! the UI over a `Channel` as MessagePack-encoded [`QueryEvent`]s; raw bytes
//! skip Tauri's JSON path and arrive in JS as an `ArrayBuffer`.
//!
//! A console reads a result a page at a time: the rest stays open on its
//! session until the UI asks for more, runs something else, or leaves it idle
//! for [`IDLE_RESULT_TIMEOUT`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use idedb_core::{
    ConnectOptions, Fetch, QueryEvent, RowChange, SchemaInfo, SchemaModel, ServerInfo, SqlProblem, TableRef,
};
use idedb_drivers::{AnyCanceller, AnySession, open_data_source};
use idedb_store::{HistoryEntry, NewHistoryEntry, Store};
use serde::Serialize;
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody, Response};

use crate::data_sources::Secrets;
use crate::error::{CommandError, CommandResult, ErrorCode};

pub type SessionId = u32;

/// Rows per page. Large enough to amortize IPC overhead, small enough that
/// the first page paints almost immediately.
const PAGE_SIZE: usize = 2000;

/// How long an open result may sit unread before its session closes it. An
/// open result holds server resources other users feel: a snapshot and table
/// locks in Postgres (blocking DDL and vacuum), a statement blocked mid-send
/// in MySQL, a shared lock on a SQLite file. Long enough to read a page and
/// scroll on; the UI offers to re-run after it.
const IDLE_RESULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct Entry {
    data_source_id: String,
    session: Arc<tokio::sync::Mutex<AnySession>>,
    canceller: AnyCanceller,
    /// Bumped whenever the session reads; an idle-close timer only acts if
    /// nothing read since it was set.
    reads: Arc<AtomicU64>,
}

impl Entry {
    /// Closes the session's open result once it has sat unread for
    /// [`IDLE_RESULT_TIMEOUT`].
    fn close_when_idle(&self) {
        let entry = self.clone();
        let seen = entry.reads.load(Ordering::SeqCst);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(IDLE_RESULT_TIMEOUT).await;
            if entry.reads.load(Ordering::SeqCst) != seen {
                return;
            }
            let mut session = entry.session.lock().await;
            // A read may have started while this waited for the lock.
            if entry.reads.load(Ordering::SeqCst) == seen {
                session.close_result().await;
            }
        });
    }
}

/// Whether the final event of a read left rows open for `fetch_more`.
fn left_open(event: &Option<QueryEvent>) -> bool {
    matches!(event, Some(QueryEvent::Done { has_more: true, .. }))
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
    let (_, session) =
        open_data_source(&store, secrets.0.as_ref(), &data_source_id, password, ConnectOptions::default()).await?;

    let id = sessions.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let server = session.server_info().clone();
    let canceller = session.canceller();
    sessions.open.lock().unwrap().insert(
        id,
        Entry {
            data_source_id,
            session: Arc::new(tokio::sync::Mutex::new(session)),
            canceller,
            reads: Arc::default(),
        },
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

/// Runs a statement. With `first_rows`, reads only that many rows and leaves
/// the rest open for `session_fetch_more`; without it, reads everything.
#[tauri::command]
pub async fn session_execute(
    id: SessionId,
    sql: String,
    first_rows: Option<usize>,
    on_event: Channel<InvokeResponseBody>,
    sessions: State<'_, Sessions>,
    store: State<'_, Store>,
) -> CommandResult<()> {
    let entry = sessions.get(id)?;
    entry.reads.fetch_add(1, Ordering::SeqCst);
    // One statement at a time per session, like a DataGrip console.
    let mut session = entry.session.lock().await;
    let started = Instant::now();
    let fetch = first_rows.map_or(Fetch::all(PAGE_SIZE), |n| Fetch::first(n, PAGE_SIZE));
    let mut outcome = None;
    session.execute(&sql, fetch, &mut relay(&on_event, &mut outcome)).await;
    drop(session);
    if left_open(&outcome) {
        entry.close_when_idle();
    }

    let (elapsed_ms, row_count, error) = match &outcome {
        Some(QueryEvent::Done { row_count, elapsed_ms, cancelled, .. }) => {
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

/// Continues the session's open result: `rows` more rows, or all of them.
#[tauri::command]
pub async fn session_fetch_more(
    id: SessionId,
    rows: Option<usize>,
    on_event: Channel<InvokeResponseBody>,
    sessions: State<'_, Sessions>,
) -> CommandResult<()> {
    let entry = sessions.get(id)?;
    entry.reads.fetch_add(1, Ordering::SeqCst);
    let mut session = entry.session.lock().await;
    let fetch = Fetch { page_size: PAGE_SIZE, limit: rows };
    let mut outcome = None;
    session.fetch_more(fetch, &mut relay(&on_event, &mut outcome)).await;
    drop(session);
    if left_open(&outcome) {
        entry.close_when_idle();
    }
    Ok(())
}

/// Releases the session's open result without reading the rest.
#[tauri::command]
pub async fn session_close_result(id: SessionId, sessions: State<'_, Sessions>) -> CommandResult<()> {
    let entry = sessions.get(id)?;
    entry.reads.fetch_add(1, Ordering::SeqCst);
    entry.session.lock().await.close_result().await;
    Ok(())
}

/// Sends every event to the UI, keeping the final one in `outcome`.
fn relay<'a>(
    channel: &'a Channel<InvokeResponseBody>,
    outcome: &'a mut Option<QueryEvent>,
) -> impl FnMut(QueryEvent) + Send + 'a {
    move |event| {
        if matches!(event, QueryEvent::Done { .. } | QueryEvent::Error { .. }) {
            *outcome = Some(event.clone());
        }
        // A send only fails once the webview is gone; nothing to report to.
        let _ = channel.send(InvokeResponseBody::Raw(encode(&event)));
    }
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

/// Applies data editor changes as one unit (inside the user's transaction
/// when one is open, without committing it). The `ApplyOutcome` goes
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

/// Validates a statement without running it. The UI calls this on a data
/// source's explorer session, never a console's, so live diagnostics never
/// queue behind a running query.
#[tauri::command]
pub async fn session_check(
    id: SessionId,
    sql: String,
    schema: Option<String>,
    sessions: State<'_, Sessions>,
) -> CommandResult<Option<SqlProblem>> {
    let session = sessions.get(id)?.session;
    let mut session = session.lock().await;
    Ok(session.check(&sql, schema.as_deref()).await?)
}

/// Sets where unqualified names resolve for a console's session.
#[tauri::command]
pub async fn session_set_schema(id: SessionId, schema: String, sessions: State<'_, Sessions>) -> CommandResult<()> {
    let session = sessions.get(id)?.session;
    let mut session = session.lock().await;
    Ok(session.set_schema(&schema).await?)
}

fn encode(event: &QueryEvent) -> Vec<u8> {
    rmp_serde::to_vec_named(event).expect("QueryEvent is always serializable")
}
