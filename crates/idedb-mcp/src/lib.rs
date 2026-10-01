//! IdeDB's MCP server: the tools an MCP client (an LLM) calls to read the
//! user's databases through IdeDB, who may call them, and the audit trail
//! they leave. The tools are plain async methods of [`McpServer`]; the
//! transport ([`McpServer::start`]: MCP's Streamable HTTP on loopback, with
//! the official SDK, rmcp) is a thin adapter that authenticates each request
//! into a [`Caller`] and calls them.
//!
//! What stands between a client and a database, in order:
//! - **Token.** [`McpServer::authenticate`] looks the token's hash up in the
//!   store on every request, so revoking a client takes effect at once.
//! - **Grants.** A client sees and uses only the data sources the user
//!   granted it, re-read from the store on every call.
//! - **Classifier.** `query` runs only what [`idedb_sql::classify`] shows is
//!   a read; `execute` runs a write only after the user approves it in
//!   IdeDB, never on a data source marked never-write, and never with a
//!   `read` grant. Transaction control, session state, files and several
//!   statements at once are refused outright.
//! - **Engine.** Reads run on pooled read-only sessions, each call inside a
//!   guard of its engine (a read-only transaction that is always rolled
//!   back, in Postgres) and under a statement timeout.
//! - **Audit.** Every call leaves one row in the store's audit log, and its
//!   result never leaves IdeDB without it.
//!
//! None of this stops what the database user itself may do with a read
//! (see [`idedb_core::ConnectOptions::read_only`]): connecting as a
//! read-only database user is what finally limits it.
//!
//! With the `bridge` feature, [`bridge`] is the other side: a stdio MCP
//! server for clients that only launch those (Claude Desktop), relaying
//! everything to this one over HTTP.

mod approvals;
#[cfg(feature = "bridge")]
pub mod bridge;
mod handler;
mod http;
mod output;
mod pool;
mod run;
mod settings;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod token;
mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use idedb_store::{SecretStore, Store};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

pub use approvals::{ApprovalRequest, Progress};
pub use idedb_store::{AuditEntry, Decision, Transport};
pub use self::http::{MCP_PATH, ServerStatus, StartError};
pub use output::INSTRUCTIONS;
pub use settings::{DEFAULT_PORT, MAX_ROWS, McpSettings};
pub use token::{hash_token, new_token};
pub use tokio_util::sync::CancellationToken;
pub use tools::{
    AccessLevel, ColumnDescription, ConnectionInfo, DescribeTableArgs, DescribeTableOutput, EngineName, ExecuteArgs,
    ExecuteOutput, ForeignKeyDescription, ListConnectionsArgs, ListConnectionsOutput, ListSchemasArgs,
    ListSchemasOutput, ListTablesArgs, ListTablesOutput, QueryArgs, QueryOutput, Reference, ResultColumn, SchemaEntry,
    TableEntry, TableKind,
};

use crate::approvals::Approvals;
use crate::pool::Pool;

/// How often a client's `last_seen_at` is written at most: every request
/// authenticates, and the store need not hear about each one.
const TOUCH_EVERY: Duration = Duration::from_secs(30);

/// What the server needs from the app around it. Object safe: the server
/// holds an `Arc<dyn Host>`.
pub trait Host: Send + Sync + 'static {
    fn store(&self) -> &Store;
    /// Where data source passwords live (the Keychain in the app).
    fn secrets(&self) -> &dyn SecretStore;
    /// Something the UI shows happened. Called on the server's tasks, so it
    /// must not block.
    fn notify(&self, event: McpEvent);
}

/// What [`Host::notify`] reports, serialized for the UI as
/// `{"kind": "audit", ...}`, `{"kind": "approvalRequested", ...}`…
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum McpEvent {
    /// The server started or stopped listening, or failed to start.
    Status(ServerStatus),
    /// A tool call was recorded in the audit log.
    Audit(AuditEntry),
    /// A write waits for the user; answer with [`McpServer::answer_approval`].
    ApprovalRequested(ApprovalRequest),
    /// An approval stopped waiting: answered, timed out or withdrawn.
    #[serde(rename_all = "camelCase")]
    ApprovalResolved { id: u64, decision: Decision },
    /// A client authenticated or called a tool; `at` is the `last_seen_at`
    /// just stored. Sent at most every 30 s per client.
    #[serde(rename_all = "camelCase")]
    ClientSeen { client_id: String, at: String },
}

/// Who calls a tool: the client its token authenticated, and what the
/// request itself says about it (unverified). The transport fills in the
/// unverified part; grants are never cached here, each call reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub client_id: String,
    /// As registered in IdeDB.
    pub client_name: String,
    /// The `clientInfo` the client declared.
    pub client_info: Option<ClientInfo>,
    /// The MCP protocol version it speaks.
    pub protocol_version: Option<String>,
    pub transport: Transport,
    /// The legacy `Mcp-Session-Id`, or the bridge's instance id.
    pub session_key: Option<String>,
}

/// An MCP client's self-reported `clientInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: Option<String>,
}

/// A tool call that failed in a way the model can act on: the message says
/// what happened and what to do next. MCP returns it as a tool result with
/// `isError`, for the model to read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

/// Something went wrong inside IdeDB rather than with the call: the model
/// cannot fix it. MCP returns it as a JSON-RPC error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] idedb_store::Error),
    #[error("invalid MCP settings: {0}")]
    InvalidSettings(String),
    #[error("{0}")]
    Internal(String),
}

/// How a tool call failed.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Internal(#[from] Error),
}

impl From<idedb_store::Error> for CallError {
    fn from(e: idedb_store::Error) -> Self {
        Self::Internal(e.into())
    }
}

/// The MCP server's state, shared by every request. Cheap to clone.
#[derive(Clone)]
pub struct McpServer(Arc<Core>);

pub(crate) struct Core {
    host: Arc<dyn Host>,
    pool: Pool,
    approvals: Approvals,
    /// When each client was last written as seen, and what it declared.
    seen: Mutex<HashMap<String, Seen>>,
    /// The HTTP listener and its status.
    listener: http::Listener,
}

struct Seen {
    at: Instant,
    info: Option<ClientInfo>,
}

impl McpServer {
    pub fn new(host: Arc<dyn Host>) -> Self {
        Self::with_parts(host, Pool::default(), Approvals::default())
    }

    /// A server whose waiting writes report progress every `every` rather
    /// than every 15 s, for tests.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_progress_every(host: Arc<dyn Host>, every: Duration) -> Self {
        Self::with_parts(host, Pool::default(), Approvals::new(every))
    }

    fn with_parts(host: Arc<dyn Host>, pool: Pool, approvals: Approvals) -> Self {
        let listener = http::Listener::default();
        Self(Arc::new(Core { host, pool, approvals, seen: Mutex::default(), listener }))
    }

    /// The stored settings, defaults for what is missing.
    pub fn settings(&self) -> Result<McpSettings, Error> {
        self.0.settings()
    }

    /// Validates and stores the settings. Tool calls use them from the next
    /// call on; a new port takes effect when [`start`](Self::start) is
    /// called again.
    pub fn save_settings(&self, settings: &McpSettings) -> Result<(), Error> {
        settings::save(self.0.host.store(), settings)
    }

    /// The client `token` belongs to, or None when it is unknown or revoked.
    /// Looked up in the store every time, so a revoked token stops working
    /// at once. Marks the client seen (see [`McpEvent::ClientSeen`]).
    pub fn authenticate(&self, token: &str) -> Result<Option<Caller>, Error> {
        let Some(client) = self.0.host.store().mcp_client_by_token_hash(&hash_token(token))? else {
            return Ok(None);
        };
        self.0.seen(&client.id, None);
        Ok(Some(Caller {
            client_id: client.id,
            client_name: client.name,
            client_info: None,
            protocol_version: None,
            transport: Transport::Http,
            session_key: None,
        }))
    }

    /// Answers a pending approval. False when it is no longer pending (it was
    /// answered, timed out or withdrawn).
    pub fn answer_approval(&self, id: u64, approve: bool) -> bool {
        self.0.approvals.answer(id, approve)
    }

    /// Writes waiting for the user, oldest first.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.0.approvals.pending()
    }

    /// Closes every pooled session on a data source, cancelling what they
    /// run: call it when the data source is edited or deleted.
    pub async fn close_data_source(&self, data_source_id: &str) {
        self.0.pool.close(|(_, source)| source == data_source_id).await;
    }

    /// Ends everything a client has going: its pending approvals are
    /// withdrawn at once (each audited as `withdrawn`, with an
    /// [`McpEvent::ApprovalResolved`]), and its pooled sessions close,
    /// cancelling what they run. Call it right after revoking or deleting
    /// the client in the store: its token already stopped authenticating,
    /// and an approval registered in between is caught when it registers.
    pub async fn close_client(&self, client_id: &str) {
        self.0.approvals.withdraw_client(client_id);
        self.0.pool.close(|(client, _)| client == client_id).await;
    }

    /// Read-only sessions open in the pool.
    pub fn open_sessions(&self) -> usize {
        self.0.pool.len()
    }
}

impl Core {
    fn settings(&self) -> Result<McpSettings, Error> {
        Ok(settings::load(self.host.store())?)
    }

    /// Records that a client was seen, at most every [`TOUCH_EVERY`], or
    /// sooner when it declares a different `clientInfo`. Best effort: a
    /// failure to record it never fails the request.
    fn seen(&self, client_id: &str, info: Option<&ClientInfo>) {
        let now = Instant::now();
        {
            let mut seen = self.seen.lock().unwrap();
            let last = seen.get(client_id);
            let due = last.is_none_or(|last| {
                now.duration_since(last.at) >= TOUCH_EVERY || info.is_some_and(|info| last.info.as_ref() != Some(info))
            });
            if !due {
                return;
            }
            let info = info.cloned().or_else(|| last.and_then(|last| last.info.clone()));
            seen.insert(client_id.to_owned(), Seen { at: now, info });
        }
        let (name, version) = info.map_or((None, None), |info| (Some(info.name.as_str()), info.version.as_deref()));
        if let Ok(Some(at)) = self.host.store().mcp_touch_client(client_id, name, version) {
            self.host.notify(McpEvent::ClientSeen { client_id: client_id.to_owned(), at });
        }
    }
}
