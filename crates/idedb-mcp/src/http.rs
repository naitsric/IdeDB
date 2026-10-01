//! Serving the tools over MCP's Streamable HTTP transport, on loopback only.
//!
//! What a request goes through, outermost first:
//! - axum, listening on `127.0.0.1:<port>` and nothing else;
//! - [`authenticate`]: every request on [`MCP_PATH`] carries
//!   `Authorization: Bearer <token>`, looked up in the store each time, so a
//!   revoked token stops working at once. It becomes the request's
//!   [`Caller`](crate::Caller), completed with what the headers say about how
//!   the client reaches IdeDB;
//! - rmcp's `StreamableHttpService`, which refuses a `Host` that is not
//!   loopback and any `Origin` (DNS rebinding, web pages) with 403, and a
//!   body over 1 MiB with 413, then speaks the protocol and calls
//!   [`Handler`].
//!
//! Protocol revisions:
//! - 2026-07-28 drops `initialize` and sessions: each request carries its
//!   protocol version, client info and capabilities in `_meta`. rmcp serves
//!   these statelessly, each POST on its own, and answers in plain JSON
//!   unless the call reports progress first (then as an SSE stream).
//! - 2025-03-26 to 2025-11-25 start with `initialize` and keep a session
//!   (`Mcp-Session-Id`). rmcp's legacy session mode serves them: it
//!   remembers the client info and the negotiated version per session, and
//!   answers in SSE streams.
//!
//! The two coexist on the same endpoint: rmcp routes each request by what
//! it carries.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::serve::ListenerExt;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use rmcp::model::ErrorCode;
use rmcp::transport::streamable_http_server::session::SessionManager;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::handler::Handler;
use crate::settings::{LONGEST_CALL, McpSettings};
use crate::{McpEvent, McpServer, Transport};

/// Where the server answers MCP: `http://127.0.0.1:<port>/mcp`.
pub const MCP_PATH: &str = "/mcp";

/// Largest request body accepted. Tool arguments are small; SQL is the
/// largest of them.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The `Host` values accepted, with any port or none.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

/// How long [`McpServer::stop`] waits for open connections to finish.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// A legacy session closes after this long without traffic. rmcp's default
/// (5 minutes) counts messages, not calls in flight, so it would cut short a
/// write waiting for approval or running long.
const SESSION_IDLE: Duration = LONGEST_CALL.saturating_add(Duration::from_secs(60));

/// `bridge` when the request comes through IdeDB's stdio bridge.
const TRANSPORT_HEADER: &str = "x-idedb-transport";
/// The bridge process's own id, its session key.
const BRIDGE_INSTANCE_HEADER: &str = "x-idedb-bridge-instance";
/// A legacy session's id.
const SESSION_HEADER: &str = "mcp-session-id";

/// JSON-RPC code of a request without a valid token, from the range left
/// to implementations.
const UNAUTHORIZED: i32 = -32001;

const MISSING_TOKEN: &str = "IdeDB's MCP server needs a client token, sent as `Authorization: Bearer idedb_…`. \
                             The user creates clients and their tokens in IdeDB.";
const UNKNOWN_TOKEN: &str = "IdeDB does not know this client token, or it was revoked. The user can create a \
                             new one in IdeDB.";

/// Whether the server listens, and where; for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    /// The port it listens on, while running.
    pub port: Option<u16>,
    /// The MCP endpoint while running, e.g. `http://127.0.0.1:7412/mcp`.
    pub url: Option<String>,
    /// Why it is not running, when starting it failed.
    pub error: Option<String>,
}

impl ServerStatus {
    fn listening(addr: SocketAddr) -> Self {
        Self { running: true, port: Some(addr.port()), url: Some(format!("http://{addr}{MCP_PATH}")), error: None }
    }

    fn failed(error: &StartError) -> Self {
        Self { error: Some(error.to_string()), ..Self::default() }
    }
}

/// Why the server could not start listening.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error(
        "Port {port} is already in use on 127.0.0.1, by another program or another IdeDB. Choose another port in \
         IdeDB's MCP settings."
    )]
    PortInUse { port: u16 },
    #[error("IdeDB could not listen on 127.0.0.1:{port}: {message}")]
    Listen { port: u16, message: String },
}

impl StartError {
    fn new(port: u16, error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::AddrInUse => Self::PortInUse { port },
            _ => Self::Listen { port, message: error.to_string() },
        }
    }
}

/// The listener's state, in the server's core.
#[derive(Default)]
pub(crate) struct Listener {
    /// Held while starting or stopping, so they never interleave.
    running: tokio::sync::Mutex<Option<Running>>,
    status: std::sync::Mutex<ServerStatus>,
}

struct Running {
    /// Stops accepting connections and ends rmcp's streams.
    cancel: CancellationToken,
    sessions: Arc<LocalSessionManager>,
    /// axum's server, until every connection closed.
    task: JoinHandle<io::Result<()>>,
}

impl McpServer {
    /// Starts listening on `127.0.0.1` at the settings' port (0 picks a free
    /// one), stopping first, as [`stop`](Self::stop) does, if it listens
    /// already. Returns the address it listens on. Reports the outcome as
    /// [`McpEvent::Status`].
    pub async fn start(&self, settings: &McpSettings) -> Result<SocketAddr, StartError> {
        let mut running = self.0.listener.running.lock().await;
        if let Some(previous) = running.take() {
            self.shutdown(previous).await;
        }
        let port = settings.port;
        let (listener, addr) = match bind(port).await {
            Ok(bound) => bound,
            Err(error) => {
                let error = StartError::new(port, error);
                self.set_status(ServerStatus::failed(&error));
                return Err(error);
            }
        };
        let cancel = CancellationToken::new();
        let (router, sessions) = self.router(cancel.clone());
        let serve = axum::serve(listener, router).with_graceful_shutdown(cancel.clone().cancelled_owned());
        let task = tokio::spawn(serve.into_future());
        *running = Some(Running { cancel, sessions, task });
        self.set_status(ServerStatus::listening(addr));
        Ok(addr)
    }

    /// Stops listening: new connections are refused, open ones end once
    /// their current request does (or after 5 s), and calls in flight are
    /// cancelled. Every pending approval is withdrawn, as the server would no
    /// longer deliver its answer. Reports [`McpEvent::Status`].
    pub async fn stop(&self) {
        let mut running = self.0.listener.running.lock().await;
        match running.take() {
            Some(previous) => self.shutdown(previous).await,
            None => {
                self.0.approvals.withdraw_all();
            }
        }
        self.set_status(ServerStatus::default());
    }

    /// Whether the server listens, and where.
    pub fn status(&self) -> ServerStatus {
        self.0.listener.status.lock().unwrap().clone()
    }

    fn set_status(&self, status: ServerStatus) {
        *self.0.listener.status.lock().unwrap() = status.clone();
        self.0.host.notify(McpEvent::Status(status));
    }

    async fn shutdown(&self, running: Running) {
        // First, so each write the user was asked about says why it ended.
        self.0.approvals.withdraw_all();
        running.cancel.cancel();
        // Closing a session cancels the calls it runs.
        let ids: Vec<_> = running.sessions.sessions.read().await.keys().cloned().collect();
        for id in ids {
            let _ = running.sessions.close_session(&id).await;
        }
        let mut task = running.task;
        if tokio::time::timeout(STOP_GRACE, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }

    fn router(&self, cancel: CancellationToken) -> (Router, Arc<LocalSessionManager>) {
        let mut sessions = LocalSessionManager::default();
        sessions.session_config.keep_alive = Some(SESSION_IDLE);
        let sessions = Arc::new(sessions);
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(LOOPBACK_HOSTS)
            .enforce_origin_validation()
            .with_legacy_session_mode(true)
            .with_json_response(true)
            .with_max_request_body_bytes(MAX_BODY_BYTES)
            .with_cancellation_token(cancel);
        let server = self.clone();
        let mcp = StreamableHttpService::new(move || Ok(Handler::new(server.clone())), sessions.clone(), config);
        let router = Router::new()
            .route_service(MCP_PATH, mcp)
            .route_layer(middleware::from_fn_with_state(self.clone(), authenticate));
        (router, sessions)
    }
}

async fn bind(port: u16) -> io::Result<(impl axum::serve::Listener<Addr = SocketAddr>, SocketAddr)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let addr = listener.local_addr()?;
    // Progress reports and responses are small writes, which Nagle's
    // algorithm would hold back.
    let listener = listener.tap_io(|connection| {
        let _ = connection.set_nodelay(true);
    });
    Ok((listener, addr))
}

/// Lets a request through only with a valid client token, and gives it its
/// [`Caller`](crate::Caller). The tools read it from the HTTP request parts
/// rmcp hands them.
async fn authenticate(State(server): State<McpServer>, mut request: Request, next: Next) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return unauthorized(MISSING_TOKEN, false);
    };
    let mut caller = match server.authenticate(&token) {
        Ok(Some(caller)) => caller,
        Ok(None) => return unauthorized(UNKNOWN_TOKEN, true),
        Err(e) => {
            let message = format!("IdeDB could not check the client token: {e}");
            return json_rpc_error(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::INTERNAL_ERROR.0, &message);
        }
    };
    let headers = request.headers();
    if header_text(headers, TRANSPORT_HEADER).is_some_and(|transport| transport.eq_ignore_ascii_case("bridge")) {
        caller.transport = Transport::Bridge;
    }
    let session_key = header_text(headers, SESSION_HEADER).or_else(|| header_text(headers, BRIDGE_INSTANCE_HEADER));
    caller.session_key = session_key.map(str::to_owned);
    request.extensions_mut().insert(caller);
    next.run(request).await
}

/// The token of an `Authorization: Bearer <token>` header.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = header_text(headers, header::AUTHORIZATION.as_str())?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_owned())
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().map(str::trim).filter(|value| !value.is_empty())
}

/// 401, with the challenge of RFC 6750: `invalid_token` only when a token
/// was sent.
fn unauthorized(message: &str, token_sent: bool) -> Response {
    let mut response = json_rpc_error(StatusCode::UNAUTHORIZED, UNAUTHORIZED, message);
    let challenge =
        if token_sent { r#"Bearer realm="IdeDB", error="invalid_token""# } else { r#"Bearer realm="IdeDB""# };
    response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
    response
}

/// A JSON-RPC error without a request id: the body was not read.
fn json_rpc_error(status: StatusCode, code: i32, message: &str) -> Response {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } });
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bearer_tokens() {
        let token = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            bearer_token(&headers)
        };
        assert_eq!(token("Bearer idedb_abc").as_deref(), Some("idedb_abc"));
        assert_eq!(token("bearer  idedb_abc ").as_deref(), Some("idedb_abc"));
        assert_eq!(token("Basic dXNlcjpwYXNz"), None);
        assert_eq!(token("Bearer "), None);
        assert_eq!(token("idedb_abc"), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn a_port_in_use_says_so() {
        let error = StartError::new(7412, io::Error::from(io::ErrorKind::AddrInUse));
        assert_eq!(error, StartError::PortInUse { port: 7412 });
        assert!(error.to_string().starts_with("Port 7412 is already in use"), "{error}");
        let status = serde_json::to_value(ServerStatus::failed(&error)).unwrap();
        assert_eq!(status["running"], false);
        assert!(status["error"].as_str().unwrap().contains("7412"), "{status}");
    }

    #[test]
    fn the_ui_hears_the_status_as_a_tagged_event() {
        let listening = ServerStatus::listening(SocketAddr::from((Ipv4Addr::LOCALHOST, 7412)));
        let event = serde_json::to_value(McpEvent::Status(listening)).unwrap();
        let expected = serde_json::json!({
            "kind": "status",
            "running": true,
            "port": 7412,
            "url": "http://127.0.0.1:7412/mcp",
            "error": null,
        });
        assert_eq!(event, expected);
    }
}
