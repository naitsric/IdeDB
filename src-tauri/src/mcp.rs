//! The MCP server inside the app (crates/idedb-mcp): the [`Host`] it runs
//! on, starting it with the app, and the commands behind the UI's MCP tool
//! window and approval dialog.
//!
//! Everything the server reports reaches the UI as one event,
//! `mcp://event`, carrying an [`McpEvent`] (`{"kind": "status", ...}`,
//! `{"kind": "approvalRequested", ...}`…). A write waiting for approval
//! also brings the window to the front.

use std::sync::Arc;

use idedb_mcp::{
    ApprovalRequest, AuditEntry, Host, MCP_PATH, McpEvent, McpServer, McpSettings, ServerStatus, new_token,
};
use idedb_store::{AuditFilter, Grant, McpClient, SecretStore, Store};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, UserAttentionType};

use crate::data_sources::Secrets;
use crate::error::{CommandError, CommandResult, ErrorCode};

/// The event every [`McpEvent`] is emitted as.
const EVENT: &str = "mcp://event";

/// The window brought to the front when a write waits for approval.
const MAIN_WINDOW: &str = "main";

/// The app as the MCP server's host: the store and Keychain it manages,
/// and the UI it notifies.
pub struct TauriHost(AppHandle);

impl Host for TauriHost {
    fn store(&self) -> &Store {
        self.0.state::<Store>().inner()
    }

    fn secrets(&self) -> &dyn SecretStore {
        self.0.state::<Secrets>().inner().0.as_ref()
    }

    fn notify(&self, event: McpEvent) {
        if matches!(event, McpEvent::ApprovalRequested(_)) {
            bring_to_front(&self.0);
        }
        let _ = self.0.emit(EVENT, event);
    }
}

/// Shows the main window, focused, wherever it was (minimized, hidden or
/// behind other apps), and asks for attention (the Dock icon bounces until
/// IdeDB is active). Every call only posts to the event loop, so it never
/// blocks the server.
fn bring_to_front(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else { return };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.request_user_attention(Some(UserAttentionType::Critical));
}

/// The app's MCP server, in Tauri's managed state.
pub struct McpState(pub McpServer);

/// Creates the server (it needs the store and secrets already managed) and
/// starts it in the background if the user turned it on. A server that
/// fails to start says why in its status, for the UI; the app runs on.
pub fn setup(app: &AppHandle) {
    let server = McpServer::new(Arc::new(TauriHost(app.clone())));
    app.manage(McpState(server.clone()));
    match server.settings() {
        Ok(settings) if settings.enabled => {
            tauri::async_runtime::spawn(async move {
                // A failure is in the status, and the UI reads it from there.
                let _ = server.start(&settings).await;
            });
        }
        Ok(_) => {}
        // The settings form shows the same error when it loads them.
        Err(e) => eprintln!("MCP server not started: {e}"),
    }
}

/// What saving the settings does to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reconcile {
    /// Start listening, or listen again on another port.
    Start,
    Stop,
    /// Leave it as it is: the other settings apply from the next tool call.
    Keep,
}

/// Turned on, the server must listen on the saved port: it starts if it is
/// not listening (also after failing to) and restarts on a new port. Turned
/// off, it stops if it listens, and a start error is cleared.
fn reconcile(status: &ServerStatus, settings: &McpSettings) -> Reconcile {
    if !settings.enabled {
        return if status.running || status.error.is_some() { Reconcile::Stop } else { Reconcile::Keep };
    }
    if !status.running || status.port != Some(settings.port) {
        return Reconcile::Start;
    }
    Reconcile::Keep
}

/// The MCP endpoint on `port`, as clients are configured with it: an IP,
/// not `localhost`, which may resolve to `::1` first.
fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}{MCP_PATH}")
}

impl From<idedb_mcp::Error> for CommandError {
    fn from(e: idedb_mcp::Error) -> Self {
        match e {
            idedb_mcp::Error::Store(e) => e.into(),
            idedb_mcp::Error::InvalidSettings(message) => {
                Self::new(ErrorCode::InvalidParams, format!("Invalid MCP settings: {message}."))
            }
            idedb_mcp::Error::Internal(message) => Self::new(ErrorCode::Storage, message),
        }
    }
}

fn client_not_found() -> CommandError {
    CommandError::new(ErrorCode::NotFound, "MCP client not found")
}

/// A client name without surrounding whitespace, never empty.
fn client_name(name: &str) -> CommandResult<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CommandError::new(ErrorCode::InvalidParams, "A client needs a name."));
    }
    Ok(name)
}

/// A client with its new token, which is never shown again.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientWithToken {
    client: McpClient,
    token: String,
}

#[derive(Serialize)]
pub struct Endpoint {
    url: String,
}

// Commands are async so the store and Keychain never block the main thread.

#[tauri::command]
pub async fn mcp_status(mcp: State<'_, McpState>) -> CommandResult<ServerStatus> {
    Ok(mcp.0.status())
}

#[tauri::command]
pub async fn mcp_settings_get(mcp: State<'_, McpState>) -> CommandResult<McpSettings> {
    Ok(mcp.0.settings()?)
}

/// Validates and saves the settings, then starts, restarts or stops the
/// server to match them. Returns its status; a failure to start is in it,
/// not an error.
#[tauri::command]
pub async fn mcp_settings_save(settings: McpSettings, mcp: State<'_, McpState>) -> CommandResult<ServerStatus> {
    let server = &mcp.0;
    server.save_settings(&settings)?;
    match reconcile(&server.status(), &settings) {
        Reconcile::Start => {
            let _ = server.start(&settings).await;
        }
        Reconcile::Stop => server.stop().await,
        Reconcile::Keep => {}
    }
    Ok(server.status())
}

/// Revoked ones included, oldest first.
#[tauri::command]
pub async fn mcp_clients_list(store: State<'_, Store>) -> CommandResult<Vec<McpClient>> {
    Ok(store.mcp_clients()?)
}

/// Registers a client, with no grants, and returns its token: the only time
/// it is seen, since the store keeps only its hash.
#[tauri::command]
pub async fn mcp_client_create(name: String, store: State<'_, Store>) -> CommandResult<ClientWithToken> {
    let (token, hash, prefix) = new_token();
    let client = store.mcp_client_create(client_name(&name)?, &hash, &prefix)?;
    Ok(ClientWithToken { client, token })
}

#[tauri::command]
pub async fn mcp_client_rename(id: String, name: String, store: State<'_, Store>) -> CommandResult<McpClient> {
    store.mcp_client_rename(&id, client_name(&name)?)?.ok_or_else(client_not_found)
}

/// Replaces the client's token and returns the new one. The old one stops
/// working at once and, since a new token usually means the old one went
/// somewhere it should not, whatever came in with it ends too: its pending
/// approvals are withdrawn and its sessions closed.
#[tauri::command]
pub async fn mcp_client_rotate(
    id: String,
    store: State<'_, Store>,
    mcp: State<'_, McpState>,
) -> CommandResult<ClientWithToken> {
    let (token, hash, prefix) = new_token();
    let client = store.mcp_client_rotate(&id, &hash, &prefix)?.ok_or_else(|| {
        CommandError::new(ErrorCode::NotFound, "MCP client not found, or revoked: a revoked client gets no new token")
    })?;
    mcp.0.close_client(&id).await;
    Ok(ClientWithToken { client, token })
}

/// Its token stops working at once; its pending approvals are withdrawn and
/// its sessions closed. The client stays listed, revoked.
#[tauri::command]
pub async fn mcp_client_revoke(
    id: String,
    store: State<'_, Store>,
    mcp: State<'_, McpState>,
) -> CommandResult<McpClient> {
    let client = store.mcp_client_revoke(&id)?;
    mcp.0.close_client(&id).await;
    client.ok_or_else(client_not_found)
}

/// Like revoking, and the client and its grants are gone. Its audit rows
/// stay, under its name.
#[tauri::command]
pub async fn mcp_client_delete(id: String, store: State<'_, Store>, mcp: State<'_, McpState>) -> CommandResult<()> {
    store.mcp_client_delete(&id)?;
    mcp.0.close_client(&id).await;
    Ok(())
}

/// Replaces all of the client's grants. Its sessions close, so none stays
/// open on a data source it lost, and its pending approvals are withdrawn
/// (a write approved after this would be refused anyway if its grant went).
#[tauri::command]
pub async fn mcp_grants_set(
    client_id: String,
    grants: Vec<Grant>,
    store: State<'_, Store>,
    mcp: State<'_, McpState>,
) -> CommandResult<McpClient> {
    let client = store.mcp_set_grants(&client_id, &grants)?;
    mcp.0.close_client(&client_id).await;
    client.ok_or_else(client_not_found)
}

/// The data sources marked never-write.
#[tauri::command]
pub async fn mcp_never_write_list(store: State<'_, Store>) -> CommandResult<Vec<String>> {
    let mut ids = Vec::new();
    for source in store.list()? {
        if store.mcp_never_write(&source.id)? {
            ids.push(source.id);
        }
    }
    Ok(ids)
}

/// Marks a data source never-write, for every client, or clears the mark.
/// Calls in flight read it again before writing.
#[tauri::command]
pub async fn mcp_never_write_set(data_source_id: String, value: bool, store: State<'_, Store>) -> CommandResult<()> {
    if !store.mcp_set_never_write(&data_source_id, value)? {
        return Err(CommandError::new(ErrorCode::NotFound, "data source not found"));
    }
    Ok(())
}

/// Newest first.
#[tauri::command]
pub async fn mcp_audit_list(filter: AuditFilter, store: State<'_, Store>) -> CommandResult<Vec<AuditEntry>> {
    Ok(store.mcp_audit(&filter)?)
}

/// False when the approval is no longer pending (answered, timed out or
/// withdrawn meanwhile).
#[tauri::command]
pub async fn mcp_approval_answer(id: u64, approve: bool, mcp: State<'_, McpState>) -> CommandResult<bool> {
    Ok(mcp.0.answer_approval(id, approve))
}

/// Writes waiting for the user, oldest first: what a reloaded UI asks again.
#[tauri::command]
pub async fn mcp_approvals_pending(mcp: State<'_, McpState>) -> CommandResult<Vec<ApprovalRequest>> {
    Ok(mcp.0.pending_approvals())
}

/// Where clients reach the server: the port it listens on, else the saved
/// one.
#[tauri::command]
pub async fn mcp_endpoint(mcp: State<'_, McpState>) -> CommandResult<Endpoint> {
    let port = match mcp.0.status().port {
        Some(port) => port,
        None => mcp.0.settings()?.port,
    };
    Ok(Endpoint { url: endpoint(port) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listening(port: u16) -> ServerStatus {
        ServerStatus { running: true, port: Some(port), url: Some(endpoint(port)), error: None }
    }

    fn failed() -> ServerStatus {
        ServerStatus { error: Some("Port 7412 is already in use".into()), ..ServerStatus::default() }
    }

    fn settings(enabled: bool, port: u16) -> McpSettings {
        McpSettings { enabled, port, ..McpSettings::default() }
    }

    #[test]
    fn turning_on_starts_and_a_new_port_restarts() {
        let stopped = ServerStatus::default();
        assert_eq!(reconcile(&stopped, &settings(true, 7412)), Reconcile::Start);
        assert_eq!(reconcile(&listening(7412), &settings(true, 7413)), Reconcile::Start);
        // Saving again after a failed start retries it.
        assert_eq!(reconcile(&failed(), &settings(true, 7412)), Reconcile::Start);
    }

    #[test]
    fn other_settings_leave_a_listening_server_alone() {
        let saved = McpSettings { max_rows: 50, approval_timeout_secs: 30, ..settings(true, 7412) };
        assert_eq!(reconcile(&listening(7412), &saved), Reconcile::Keep);
    }

    #[test]
    fn turning_off_stops_and_clears_a_start_error() {
        assert_eq!(reconcile(&listening(7412), &settings(false, 7412)), Reconcile::Stop);
        assert_eq!(reconcile(&failed(), &settings(false, 7412)), Reconcile::Stop);
        assert_eq!(reconcile(&ServerStatus::default(), &settings(false, 7500)), Reconcile::Keep);
    }

    #[test]
    fn the_endpoint_is_on_the_loopback_ip() {
        assert_eq!(endpoint(7412), "http://127.0.0.1:7412/mcp");
    }

    #[test]
    fn client_names_are_trimmed_and_required() {
        assert_eq!(client_name("  claude-code ").unwrap(), "claude-code");
        let empty = client_name(" \t").unwrap_err();
        assert!(matches!(empty.code, ErrorCode::InvalidParams));
    }

    #[test]
    fn invalid_settings_read_as_invalid_params() {
        let error = CommandError::from(idedb_mcp::Error::InvalidSettings("the port must be between 1 and 65535".into()));
        assert!(matches!(error.code, ErrorCode::InvalidParams));
        assert_eq!(error.message, "Invalid MCP settings: the port must be between 1 and 65535.");
        let store = CommandError::from(idedb_mcp::Error::Store(idedb_store::Error::Secret("denied".into())));
        assert!(matches!(store.code, ErrorCode::Storage));
    }
}
