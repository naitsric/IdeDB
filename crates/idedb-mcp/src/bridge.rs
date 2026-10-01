//! The stdio bridge, `idedb mcp-bridge`: an MCP server over stdio for the
//! clients that only launch those (Claude Desktop), relaying everything to
//! IdeDB's Streamable HTTP server on `127.0.0.1`. The client starts it as a
//! child process and writes one JSON-RPC message per line to its stdin; the
//! bridge POSTs each one to `http://127.0.0.1:<port>/mcp` and writes back,
//! one per line on its stdout, the messages IdeDB answers with. It decides
//! nothing itself: the token, grants, approvals and audit are IdeDB's, and
//! the audit log shows which calls came through the bridge.
//!
//! Every POST carries:
//! - `Authorization: Bearer <token>`, the token in `IDEDB_MCP_TOKEN` (never
//!   an argument: other processes can read those);
//! - `X-IdeDB-Transport: bridge`, and `X-IdeDB-Bridge-Instance` with an id
//!   of this process: the calls' session key when there is no MCP session;
//! - what the protocol revision in use asks for:
//!   - 2025-03-26 to 2025-11-25: `initialize` opens a session. The bridge
//!     keeps its `Mcp-Session-Id` and the negotiated version, and sends both
//!     (the version as `MCP-Protocol-Version`) with every later message. If
//!     IdeDB no longer knows the session (404: the app or its server
//!     restarted), the bridge opens a new one with the client's own
//!     `initialize` and sends the message again, so the client never
//!     notices.
//!   - 2026-07-28: no session. Each request names its version in `_meta`,
//!     which the bridge repeats as `MCP-Protocol-Version`, along with its
//!     method and name as `Mcp-Method` and `Mcp-Name` (SEP-2243).
//!     Notifications carry the version the last request named. None of
//!     IdeDB's tools promotes an argument to a header (`x-mcp-header`), so
//!     there is no `Mcp-Param-*` to send.
//!
//! IdeDB answers a request with JSON, written as it is, or with an SSE
//! stream (always in a session; outside one, once a call reports progress),
//! written event by event: a call's progress notifications, then its
//! result. A notification gets a 202 and nothing goes back. What keeps a
//! request from IdeDB's tools (no token, a rejected one, IdeDB not running,
//! a broken connection…) becomes a JSON-RPC error for it that says what to
//! do. Logs go to stderr, which clients keep in their MCP logs: stdout is
//! the protocol's alone.
//!
//! Requests run concurrently, each on its own connection, so a write can
//! wait minutes for the user's approval while reads go on. Lines leave
//! through a single writer and never interleave. A `notifications/cancelled`
//! is sent on to IdeDB, which cancels the call in a session, and the bridge
//! drops the call's connection, which cancels it outside one; either way a
//! write waiting for approval is withdrawn, and nothing is written for the
//! cancelled request.
//!
//! The bridge never opens the standalone GET stream, because IdeDB sends
//! nothing there: a call's progress travels on the call's own stream (rmcp
//! routes it by progress token), and the server neither makes requests of
//! its own (sampling, elicitation, roots) nor announces list changes.
//!
//! When nothing listens on the port, the bridge opens IdeDB in the
//! background (once per process, and only on macOS) and keeps trying for
//! up to 20 s. After that, or if IdeDB couldn't be opened, requests get an
//! error saying to turn the MCP server on.
//!
//! When stdin ends, the client is done with the bridge, and may stop it
//! soon. The bridge gives calls in flight a moment to finish, cancels the
//! rest (a session's in IdeDB, with `notifications/cancelled`: closing the
//! session alone would let them run 5 s more, approval dialogs up), ends
//! the session (DELETE) and returns.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Response, StatusCode};
use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{OnceCell, mpsc};
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;

use crate::http::{BRIDGE_INSTANCE_HEADER, MCP_PATH, SESSION_HEADER, TRANSPORT_HEADER, UNAUTHORIZED};
use crate::settings::DEFAULT_PORT;

/// The client token. Read from the environment, as arguments show in `ps`.
pub const TOKEN_VAR: &str = "IDEDB_MCP_TOKEN";
/// The port, when `--port` doesn't give one.
pub const PORT_VAR: &str = "IDEDB_MCP_PORT";

/// How long to wait for IdeDB to listen, once opened.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);
/// How often to try meanwhile.
const RETRY_EVERY: Duration = Duration::from_millis(250);
/// How long calls in flight get to finish once stdin ends.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// How long a connection may take to open. It is loopback: nothing
/// listening is refused at once.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const PROTOCOL_VERSION_HEADER: HeaderName = HeaderName::from_static("mcp-protocol-version");
const METHOD_HEADER: HeaderName = HeaderName::from_static("mcp-method");
const NAME_HEADER: HeaderName = HeaderName::from_static("mcp-name");
/// The first revision whose messages name their method in headers
/// (SEP-2243). Revisions are dates, so they compare as text.
const STANDARD_HEADERS: &str = "2026-07-28";
/// Where a 2026-07-28 request names its protocol version.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// Sent to open a replacement session, as the client did after its
/// `initialize`.
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

// JSON-RPC codes of the errors the bridge answers with itself.
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
/// IdeDB couldn't be reached, or answered in a way the bridge can't relay.
/// From the range left to implementations, next to the server's
/// [`UNAUTHORIZED`].
const UNAVAILABLE: i32 = -32000;

const USAGE: &str = "\
Relays MCP between a client that launches stdio servers (such as Claude
Desktop) and IdeDB's MCP server, opening IdeDB if it isn't running.

Usage: idedb mcp-bridge [--port <port>]

Options:
  --port <port>  IdeDB's MCP port on 127.0.0.1 (default: $IDEDB_MCP_PORT, or 7412)
  -h, --help     Print this help

Environment:
  IDEDB_MCP_TOKEN  The client's token, shown when the client was created in
                   IdeDB → MCP → Clients. Never an argument: other processes
                   can read those.
  IDEDB_MCP_PORT   The port, when --port isn't given.
";

/// What `idedb mcp-bridge` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Help,
    /// Relay to IdeDB on `port`, with `token` (none when unset).
    Run {
        port: u16,
        token: Option<String>,
    },
}

/// Reads the arguments after `mcp-bridge`, and the environment through
/// `var`. The error says what is wrong, for stderr.
pub fn parse_args(
    args: impl IntoIterator<Item = String>,
    var: impl Fn(&str) -> Option<String>,
) -> Result<Invocation, String> {
    let mut port = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "-h" | "--help" => return Ok(Invocation::Help),
            "--port" => {
                let value = value.or_else(|| args.next()).ok_or("--port needs a port number")?;
                port = Some(parse_port(&value).map_err(|e| format!("--port: {e}"))?);
            }
            "--token" => {
                return Err(format!(
                    "the token is read from {TOKEN_VAR}, never from arguments, which other processes can read"
                ));
            }
            _ => return Err(format!("unexpected argument `{arg}`")),
        }
    }
    let port = match (port, var(PORT_VAR)) {
        (Some(port), _) => port,
        (None, Some(value)) if !value.trim().is_empty() => {
            parse_port(value.trim()).map_err(|e| format!("{PORT_VAR}: {e}"))?
        }
        (None, _) => DEFAULT_PORT,
    };
    let token = var(TOKEN_VAR).map(|token| token.trim().to_owned()).filter(|token| !token.is_empty());
    Ok(Invocation::Run { port, token })
}

fn parse_port(text: &str) -> Result<u16, String> {
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(format!("`{text}` is not a port (1 to 65535)")),
    }
}

/// `idedb mcp-bridge [--port <port>]`, `args` being the arguments after
/// `mcp-bridge`: relays stdin and stdout to IdeDB's MCP server until stdin
/// ends, opening the app `bundle_id` names if it isn't running. Returns the
/// exit code: 0 once stdin ended (or for `--help`), 2 for bad arguments, 1
/// when the bridge couldn't run.
pub fn main(args: impl IntoIterator<Item = String>, bundle_id: &str) -> i32 {
    let (port, token) = match parse_args(args, |name| std::env::var(name).ok()) {
        Ok(Invocation::Help) => {
            print!("{USAGE}");
            return 0;
        }
        Ok(Invocation::Run { port, token }) => (port, token),
        Err(error) => {
            eprintln!("idedb mcp-bridge: {error}\n\n{USAGE}");
            return 2;
        }
    };
    if token.is_none() {
        log(&format!("{TOKEN_VAR} is not set: every request fails until it is"));
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            log(&format!("could not start: {e}"));
            return 1;
        }
    };
    log(&format!("relaying to http://127.0.0.1:{port}{MCP_PATH}"));
    let config = BridgeConfig::new(port, token);
    let relayed = runtime.block_on(relay(tokio::io::stdin(), tokio::io::stdout(), config, OpenApp::new(bundle_id)));
    // A read of stdin or an `open` may still be blocking a thread.
    runtime.shutdown_timeout(Duration::from_millis(100));
    match relayed {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("{e}"));
            1
        }
    }
}

/// How a bridge reaches IdeDB.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// IdeDB's MCP port on `127.0.0.1`.
    pub port: u16,
    /// The client token. Without one, every request is answered with an
    /// error saying how to set it.
    pub token: Option<String>,
    /// This bridge's id, sent as `X-IdeDB-Bridge-Instance`.
    pub instance: String,
    /// How long to wait for IdeDB to listen, once opened.
    pub launch_wait: Duration,
    /// How often to try meanwhile.
    pub retry_every: Duration,
    /// How long calls in flight get to finish once stdin ends.
    pub shutdown_grace: Duration,
}

impl BridgeConfig {
    /// The defaults, with a new instance id.
    pub fn new(port: u16, token: Option<String>) -> Self {
        Self {
            port,
            token,
            instance: uuid::Uuid::new_v4().to_string(),
            launch_wait: LAUNCH_WAIT,
            retry_every: RETRY_EVERY,
            shutdown_grace: SHUTDOWN_GRACE,
        }
    }
}

/// Opens IdeDB when nothing listens on its port. Asked at most once per
/// bridge; it returns once the app is on its way, without waiting for it to
/// listen. Any `Fn() -> impl Future` is one, for tests.
pub trait Launcher: Send + Sync + 'static {
    /// Err says why IdeDB can't be opened: the bridge then doesn't wait.
    fn launch(&self) -> impl Future<Output = Result<(), String>> + Send;
}

impl<F, Fut> Launcher for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send,
{
    fn launch(&self) -> impl Future<Output = Result<(), String>> + Send {
        self()
    }
}

/// Opens an app in the background by its bundle id, with Launch Services
/// (`open -g -b`): wherever it is installed, without bringing it to the
/// front. macOS only; elsewhere it fails, and the bridge doesn't wait.
pub struct OpenApp {
    bundle_id: String,
}

impl OpenApp {
    pub fn new(bundle_id: impl Into<String>) -> Self {
        Self { bundle_id: bundle_id.into() }
    }
}

impl Launcher for OpenApp {
    #[cfg(target_os = "macos")]
    async fn launch(&self) -> Result<(), String> {
        use std::process::{Command, Stdio};

        let bundle_id = self.bundle_id.clone();
        // Never stdout: it is the protocol's.
        let open = move || {
            Command::new("/usr/bin/open")
                .args(["-g", "-b", &bundle_id])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .output()
        };
        let output = tokio::task::spawn_blocking(open).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if stderr.is_empty() {
            format!("`open -g -b {}` failed: {}", self.bundle_id, output.status)
        } else {
            stderr
        })
    }

    #[cfg(not(target_os = "macos"))]
    async fn launch(&self) -> Result<(), String> {
        Err(format!("the bridge opens {} by itself on macOS only", self.bundle_id))
    }
}

/// Relays MCP messages, one per line, from `reader` to IdeDB and IdeDB's
/// answers to `writer`, until `reader` ends; see the [module](self) docs.
/// Opens IdeDB with `launcher` when it isn't listening. Errors only when the
/// bridge can't start; whatever happens to a message is answered on
/// `writer` or logged.
pub async fn relay<R, W, L>(reader: R, writer: W, config: BridgeConfig, launcher: L) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    L: Launcher,
{
    let grace = config.shutdown_grace;
    let (lines, queued) = mpsc::unbounded_channel();
    let bridge = Arc::new(Bridge::new(config, launcher, lines)?);
    let mut writer = tokio::spawn(write_lines(writer, queued));
    let mut calls = JoinSet::new();
    // Requests in flight, by their id as JSON text (1 and "1" are different
    // ids).
    let mut in_flight: HashMap<String, (Json, AbortHandle)> = HashMap::new();
    let mut input = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                log(&format!("reading stdin: {e}"));
                break;
            }
        }
        while calls.try_join_next().is_some() {}
        let Ok(text) = std::str::from_utf8(&line) else {
            bridge.write(error_message(&Json::Null, PARSE_ERROR, "Parse error: the line is not UTF-8"));
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let message = match Message::parse(text.to_owned()) {
            Ok(message) => message,
            Err((code, error)) => {
                bridge.write(error_message(&Json::Null, code, &error));
                continue;
            }
        };
        if message.is_barrier() {
            bridge.handle(message).await;
            continue;
        }
        if let Some(cancelled) = &message.cancels
            && let Some((_, call)) = in_flight.remove(&cancelled.to_string())
        {
            // Dropping its connection cancels a call outside a session; in
            // one, the notification does.
            call.abort();
        }
        let id = message.id.clone();
        let call = calls.spawn({
            let bridge = bridge.clone();
            async move { bridge.handle(message).await }
        });
        if let Some(id) = id {
            in_flight.retain(|_, (_, call)| !call.is_finished());
            in_flight.insert(id.to_string(), (id, call));
        }
    }
    // The client closed stdin: it is done, and may stop the bridge soon.
    let _ = tokio::time::timeout(grace, async { while calls.join_next().await.is_some() {} }).await;
    let left = in_flight.into_values().filter(|(_, call)| !call.is_finished()).map(|(id, _)| id);
    bridge.cancel_in_session(left.collect()).await;
    calls.shutdown().await;
    bridge.close_session().await;
    drop(bridge);
    // What is queued still goes out, unless the client stopped reading.
    if tokio::time::timeout(grace, &mut writer).await.is_err() {
        writer.abort();
    }
    Ok(())
}

/// Writes each line whole, in the order queued, until every sender is gone
/// or the client stops reading.
async fn write_lines<W: AsyncWrite + Unpin>(mut writer: W, mut lines: mpsc::UnboundedReceiver<String>) {
    while let Some(mut line) = lines.recv().await {
        line.push('\n');
        let written = async {
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await
        };
        if let Err(e) = written.await {
            log(&format!("writing stdout: {e}"));
            return;
        }
    }
}

/// A line from the client, and what routing it takes.
#[derive(Debug)]
struct Message {
    /// As the client wrote it, which is what IdeDB gets.
    text: String,
    /// For requests and notifications.
    method: Option<String>,
    /// A request's id; None for notifications and responses.
    id: Option<Json>,
    /// What SEP-2243 puts in `Mcp-Name`: the tool's or prompt's name, the
    /// resource's URI or the task's id.
    name: Option<String>,
    /// The protocol version a 2026-07-28 request names in its `_meta`.
    version: Option<String>,
    /// The request a `notifications/cancelled` cancels.
    cancels: Option<Json>,
}

impl Message {
    /// Err has the JSON-RPC code and message to answer a bad line with.
    fn parse(text: String) -> Result<Self, (i32, String)> {
        let value: Json = serde_json::from_str(&text).map_err(|e| (PARSE_ERROR, format!("Parse error: {e}")))?;
        let Json::Object(message) = &value else {
            let error = "Invalid request: one JSON-RPC message per line (batches are not supported)";
            return Err((INVALID_REQUEST, error.into()));
        };
        let method = message.get("method").and_then(Json::as_str).map(str::to_owned);
        let id = method.as_ref().and(message.get("id")).filter(|id| !id.is_null()).cloned();
        let params = message.get("params");
        let param = |key: &str| params.and_then(|p| p.get(key)).and_then(Json::as_str).map(str::to_owned);
        let name = match method.as_deref() {
            Some("tools/call" | "prompts/get") => param("name"),
            Some("resources/read" | "resources/subscribe" | "resources/unsubscribe") => param("uri"),
            Some("tasks/get" | "tasks/update" | "tasks/cancel") => param("taskId"),
            _ => None,
        };
        let meta = id.as_ref().and(params).and_then(|params| params.get("_meta"));
        let version = meta.and_then(|meta| meta.get(META_PROTOCOL_VERSION)).and_then(Json::as_str).map(str::to_owned);
        let cancels = match method.as_deref() {
            Some("notifications/cancelled") => params.and_then(|p| p.get("requestId")).cloned(),
            _ => None,
        };
        Ok(Self { text, method, id, name, version, cancels })
    }

    fn is_initialize(&self) -> bool {
        self.id.is_some() && self.method.as_deref() == Some("initialize")
    }

    /// Nothing may overtake these: a session must be open and initialized
    /// before anything else reaches IdeDB in it.
    fn is_barrier(&self) -> bool {
        self.is_initialize() || (self.id.is_none() && self.method.as_deref() == Some("notifications/initialized"))
    }

    /// For logs.
    fn label(&self) -> String {
        match (&self.method, &self.id) {
            (Some(method), Some(id)) => format!("{method} (id {id})"),
            (Some(method), None) => method.clone(),
            (None, _) => "a response".into(),
        }
    }
}

/// Why IdeDB's tools never answered a message.
#[derive(Debug)]
enum Failure {
    /// [`TOKEN_VAR`] is not set.
    NoToken,
    /// IdeDB doesn't know the token, or it was revoked.
    Unauthorized,
    /// Nothing listens on the port, even after opening IdeDB.
    NotRunning,
    /// The connection broke before the answer was in.
    Connection(String),
    /// IdeDB answered with no message the bridge could relay.
    Status { status: StatusCode, body: String },
    /// The answer ended without the response to the request.
    Unanswered,
    /// IdeDB forgot the session, and a new one couldn't be opened.
    SessionLost(String),
}

impl Failure {
    fn code(&self) -> i32 {
        match self {
            Self::NoToken | Self::Unauthorized => UNAUTHORIZED,
            _ => UNAVAILABLE,
        }
    }

    /// What happened and what to do, for the client to show.
    fn message(&self, port: u16) -> String {
        match self {
            Self::NoToken => format!(
                "{TOKEN_VAR} is not set, so IdeDB can't tell which client this is. Create a client in IdeDB → MCP → \
                 Clients and set its token as {TOKEN_VAR} in the env of this MCP server's configuration."
            ),
            Self::Unauthorized => format!(
                "IdeDB rejected the client token: it is wrong or was revoked. Create a new one in IdeDB → MCP → \
                 Clients and set it as {TOKEN_VAR} in this MCP server's configuration."
            ),
            Self::NotRunning => format!(
                "IdeDB isn't running or its MCP server is off. Open IdeDB → MCP → Server and turn it on (port {port})."
            ),
            Self::Connection(error) => format!(
                "The connection to IdeDB broke before it answered ({error}). IdeDB may have quit or stopped its MCP \
                 server."
            ),
            Self::Status { status, body } => match *status {
                StatusCode::FORBIDDEN => format!(
                    "IdeDB refused the request (HTTP 403{}). Check that {port} is IdeDB's MCP port, in IdeDB → MCP → \
                     Server.",
                    detail(body)
                ),
                StatusCode::PAYLOAD_TOO_LARGE => {
                    "The message is larger than IdeDB's MCP server accepts (1 MiB). Send a shorter statement.".into()
                }
                _ => format!("IdeDB answered HTTP {status}{}.", detail(body)),
            },
            Self::Unanswered => {
                "IdeDB ended its answer without responding. It may have quit or stopped its MCP server.".into()
            }
            Self::SessionLost(why) => format!(
                "IdeDB no longer knows this MCP session (the app or its MCP server restarted), and a new one couldn't \
                 be opened: {why}"
            ),
        }
    }
}

/// `: <body>` for a message, at most 200 characters of it.
fn detail(body: &str) -> String {
    let body = body.trim();
    match body.char_indices().nth(200) {
        _ if body.is_empty() => String::new(),
        Some((end, _)) => format!(": {}…", &body[..end]),
        None => format!(": {body}"),
    }
}

fn connection(error: reqwest::Error) -> Failure {
    Failure::Connection(describe(&error))
}

/// The bridge's state, shared by the calls in flight.
struct Bridge<L> {
    config: BridgeConfig,
    url: String,
    http: Client,
    launcher: L,
    /// Until when IdeDB is waited for: set the first time it can't be
    /// reached, once it was opened.
    launched: OnceCell<Instant>,
    session: Mutex<Session>,
    /// Held while a session replaces one IdeDB forgot, so the calls that
    /// found it gone open only one.
    reopening: tokio::sync::Mutex<()>,
    /// To the writer.
    lines: mpsc::UnboundedSender<String>,
}

/// What the client's messages so far established.
#[derive(Debug, Default)]
struct Session {
    /// The `Mcp-Session-Id` IdeDB answered `initialize` with.
    id: Option<String>,
    /// The version `initialize` negotiated, or else the last one a request
    /// named in its `_meta`.
    version: Option<String>,
    /// The client's `initialize`, to open a new session with.
    initialize: Option<String>,
}

impl<L: Launcher> Bridge<L> {
    fn new(config: BridgeConfig, launcher: L, lines: mpsc::UnboundedSender<String>) -> io::Result<Self> {
        let http = Client::builder()
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .tcp_nodelay(true)
            .user_agent(concat!("idedb-mcp-bridge/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(io::Error::other)?;
        let url = format!("http://127.0.0.1:{}{MCP_PATH}", config.port);
        Ok(Self {
            config,
            url,
            http,
            launcher,
            launched: OnceCell::new(),
            session: Mutex::default(),
            reopening: tokio::sync::Mutex::default(),
            lines,
        })
    }

    fn write(&self, line: String) {
        // The writer only stops once the client stopped reading.
        let _ = self.lines.send(line);
    }

    /// Relays `message` and IdeDB's answer. A failure becomes the request's
    /// error; for a notification, a line in the log.
    async fn handle(&self, message: Message) {
        let Err(failure) = self.exchange(&message).await else { return };
        let text = failure.message(self.config.port);
        log(&format!("{}: {text}", message.label()));
        if let Some(id) = &message.id {
            self.write(error_message(id, failure.code(), &text));
        }
    }

    async fn exchange(&self, message: &Message) -> Result<(), Failure> {
        let auth = self.auth()?;
        let (session, version) = self.route(message);
        let mut response = self.post(&auth, message, session.as_deref(), version.as_deref()).await?;
        if response.status() == StatusCode::NOT_FOUND
            && let Some(stale) = &session
        {
            let (session, version) = self.reopen(&auth, stale).await?;
            response = self.post(&auth, message, Some(&session), version.as_deref()).await?;
        }
        let opened = message.is_initialize() && response.status().is_success();
        if opened {
            let mut session = self.session.lock().unwrap();
            *session = Session {
                id: header_text(response.headers(), SESSION_HEADER),
                version: None,
                initialize: Some(message.text.clone()),
            };
        }
        let answer = self.answer(message, response).await?;
        if opened {
            let version = answer.as_ref().and_then(negotiated_version);
            let mut session = self.session.lock().unwrap();
            if let Some(id) = &session.id {
                log(&format!("session {id} opened, protocol {}", version.as_deref().unwrap_or("unknown")));
            }
            session.version = version;
        }
        Ok(())
    }

    /// The `Authorization` header.
    fn auth(&self) -> Result<HeaderValue, Failure> {
        let token = self.config.token.as_deref().ok_or(Failure::NoToken)?;
        // IdeDB's tokens are plain ASCII: anything else is not one of them.
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Failure::Unauthorized)?;
        value.set_sensitive(true);
        Ok(value)
    }

    /// The session's id and protocol version.
    fn session_now(&self) -> (Option<String>, Option<String>) {
        let session = self.session.lock().unwrap();
        (session.id.clone(), session.version.clone())
    }

    /// The session and protocol version to send `message` with.
    fn route(&self, message: &Message) -> (Option<String>, Option<String>) {
        if message.is_initialize() {
            return (None, None);
        }
        let mut session = self.session.lock().unwrap();
        if let Some(version) = &message.version {
            // 2026-07-28: no session, and notifications follow the version
            // requests name.
            if session.id.is_none() {
                session.version = Some(version.clone());
            }
            return (None, Some(version.clone()));
        }
        (session.id.clone(), session.version.clone())
    }

    fn headers(&self, auth: &HeaderValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, auth.clone());
        headers.insert(HeaderName::from_static(TRANSPORT_HEADER), HeaderValue::from_static("bridge"));
        if let Ok(instance) = HeaderValue::from_str(&self.config.instance) {
            headers.insert(HeaderName::from_static(BRIDGE_INSTANCE_HEADER), instance);
        }
        headers
    }

    fn message_headers(
        &self,
        auth: &HeaderValue,
        message: &Message,
        session: Option<&str>,
        version: Option<&str>,
    ) -> HeaderMap {
        let mut headers = self.headers(auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/event-stream"));
        let mut insert = |name: HeaderName, value: Option<HeaderValue>| {
            if let Some(value) = value {
                headers.insert(name, value);
            }
        };
        insert(HeaderName::from_static(SESSION_HEADER), session.and_then(|id| HeaderValue::from_str(id).ok()));
        if let Some(version) = version {
            insert(PROTOCOL_VERSION_HEADER, HeaderValue::from_str(version).ok());
            if version >= STANDARD_HEADERS {
                insert(METHOD_HEADER, message.method.as_deref().and_then(standard_header_value));
                insert(NAME_HEADER, message.name.as_deref().and_then(standard_header_value));
            }
        }
        headers
    }

    /// POSTs `message`. When IdeDB isn't listening, opens it and tries again
    /// for a while.
    async fn post(
        &self,
        auth: &HeaderValue,
        message: &Message,
        session: Option<&str>,
        version: Option<&str>,
    ) -> Result<Response, Failure> {
        let headers = self.message_headers(auth, message, session, version);
        loop {
            let request = self.http.post(&self.url).headers(headers.clone()).body(message.text.clone());
            match request.send().await {
                Ok(response) => return Ok(response),
                Err(e) if e.is_connect() => self.wait_for_idedb(&e).await?,
                Err(e) => return Err(connection(e)),
            }
        }
    }

    /// Waits a moment for IdeDB, which isn't listening; the first time,
    /// after opening it. Fails once the wait is over.
    async fn wait_for_idedb(&self, error: &reqwest::Error) -> Result<(), Failure> {
        let until = *self
            .launched
            .get_or_init(|| async {
                log(&format!("IdeDB isn't answering on port {} ({}); opening it", self.config.port, describe(error)));
                match self.launcher.launch().await {
                    Ok(()) => Instant::now() + self.config.launch_wait,
                    Err(why) => {
                        log(&format!("could not open IdeDB: {why}"));
                        Instant::now()
                    }
                }
            })
            .await;
        let now = Instant::now();
        if now >= until {
            return Err(Failure::NotRunning);
        }
        tokio::time::sleep(self.config.retry_every.min(until - now)).await;
        Ok(())
    }

    /// Writes what IdeDB answered `message` with, and returns the response
    /// to it when it is a request.
    async fn answer(&self, message: &Message, response: Response) -> Result<Option<Json>, Failure> {
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(Failure::Unauthorized);
        }
        let Some(id) = &message.id else {
            // A notification or a response: accepted with 202, and nothing
            // goes back to the client.
            if status.is_success() {
                return Ok(None);
            }
            return Err(Failure::Status { status, body: response.text().await.unwrap_or_default() });
        };
        if !status.is_success() {
            let body = response.text().await.map_err(connection)?;
            // rmcp refuses a request (a header that doesn't match, an unknown
            // method…) with a JSON-RPC error for it: that is the answer.
            let Some(mut error) = json_rpc_answer(&body) else {
                return Err(Failure::Status { status, body });
            };
            if error.get("id").is_none_or(Json::is_null) {
                error["id"] = id.clone();
            }
            self.write(error.to_string());
            return Ok(Some(error));
        }
        match read_answer(response, Some(id), |line| self.write(line)).await? {
            Some(answer) => Ok(Some(answer)),
            None => Err(Failure::Unanswered),
        }
    }

    /// Opens a session in place of `stale`, which IdeDB no longer knows,
    /// with the client's own `initialize`, and returns its id and protocol
    /// version. Calls that found `stale` gone meanwhile get the same one.
    async fn reopen(&self, auth: &HeaderValue, stale: &str) -> Result<(String, Option<String>), Failure> {
        let _one_at_a_time = self.reopening.lock().await;
        let initialize = {
            let session = self.session.lock().unwrap();
            match &session.id {
                Some(id) if id != stale => return Ok((id.clone(), session.version.clone())),
                _ => session.initialize.clone(),
            }
        };
        let initialize = initialize
            .and_then(|text| Message::parse(text).ok())
            .ok_or_else(|| Failure::SessionLost("the client's initialize request wasn't seen".into()))?;
        log(&format!("IdeDB no longer knows session {stale}; opening a new one"));
        let response = self.post(auth, &initialize, None, None).await?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(Failure::Unauthorized),
            status if !status.is_success() => {
                return Err(Failure::SessionLost(format!("IdeDB answered initialize with HTTP {status}")));
            }
            _ => {}
        }
        let id = header_text(response.headers(), SESSION_HEADER)
            .ok_or_else(|| Failure::SessionLost("IdeDB opened no session".into()))?;
        let answer = read_answer(response, initialize.id.as_ref(), |_| {}).await?;
        let answer = answer.ok_or_else(|| Failure::SessionLost("IdeDB didn't answer initialize".into()))?;
        if let Some(error) = answer.get("error") {
            return Err(Failure::SessionLost(format!("IdeDB refused initialize: {error}")));
        }
        let version = negotiated_version(&answer);
        let initialized = Message::parse(INITIALIZED.into()).expect("a valid notification");
        let response = self.post(auth, &initialized, Some(&id), version.as_deref()).await?;
        if !response.status().is_success() {
            return Err(Failure::SessionLost(format!(
                "IdeDB answered notifications/initialized with HTTP {}",
                response.status()
            )));
        }
        log(&format!("session {id} opened, protocol {}", version.as_deref().unwrap_or("unknown")));
        let mut session = self.session.lock().unwrap();
        session.id = Some(id.clone());
        session.version = version.clone();
        Ok((id, version))
    }

    /// Cancels the requests `ids`, in flight in the session, if any, at
    /// once and all together: what the client would do if it still could.
    /// IdeDB withdraws the writes they wait to have approved. Outside a
    /// session, dropping their connections does it.
    async fn cancel_in_session(&self, ids: Vec<Json>) {
        let (Some(session), version) = self.session_now() else { return };
        let Ok(auth) = self.auth() else { return };
        let mut cancels = JoinSet::new();
        for id in ids {
            let params = json!({ "requestId": id, "reason": "The MCP client closed the bridge's input." });
            let notification = json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": params });
            let Ok(message) = Message::parse(notification.to_string()) else { continue };
            let headers = self.message_headers(&auth, &message, Some(&session), version.as_deref());
            let post =
                self.http.post(&self.url).headers(headers).body(message.text).timeout(self.config.shutdown_grace);
            cancels.spawn(post.send());
        }
        while let Some(sent) = cancels.join_next().await {
            if let Ok(Err(e)) = sent {
                log(&format!("cancelling a call in session {session}: {}", describe(&e)));
            }
        }
    }

    /// Ends the session, if any.
    async fn close_session(&self) {
        let (Some(id), version) = self.session_now() else { return };
        let Ok(auth) = self.auth() else { return };
        let mut headers = self.headers(&auth);
        if let Ok(value) = HeaderValue::from_str(&id) {
            headers.insert(HeaderName::from_static(SESSION_HEADER), value);
        }
        if let Some(value) = version.and_then(|version| HeaderValue::from_str(&version).ok()) {
            headers.insert(PROTOCOL_VERSION_HEADER, value);
        }
        let delete = self.http.delete(&self.url).headers(headers).timeout(self.config.shutdown_grace);
        match delete.send().await {
            Ok(response) if response.status().is_success() => log(&format!("session {id} ended")),
            Ok(response) => log(&format!("ending session {id}: HTTP {}", response.status())),
            Err(e) => log(&format!("ending session {id}: {}", describe(&e))),
        }
    }
}

/// Reads a successful answer, JSON or SSE, handing each message in it to
/// `each` as one line, as it arrives. Returns the response to `id`, once it
/// arrives; None if the answer ended without it.
async fn read_answer(
    mut response: Response,
    id: Option<&Json>,
    mut each: impl FnMut(String),
) -> Result<Option<Json>, Failure> {
    let content_type = header_text(response.headers(), CONTENT_TYPE.as_str()).unwrap_or_default();
    if content_type.starts_with("text/event-stream") {
        let mut events = SseParser::default();
        while let Some(chunk) = response.chunk().await.map_err(connection)? {
            for data in events.feed(&chunk) {
                let Some((line, message)) = json_line(&data) else {
                    log(&format!("skipped an event that is not JSON: {}", detail(&data)));
                    continue;
                };
                let answers = is_answer(&message, id);
                each(line);
                if answers {
                    return Ok(Some(message));
                }
            }
        }
        return Ok(None);
    }
    let status = response.status();
    let body = response.text().await.map_err(connection)?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    let Some((line, message)) = json_line(&body) else {
        return Err(Failure::Status { status, body });
    };
    let answers = is_answer(&message, id);
    each(line);
    Ok(answers.then_some(message))
}

/// Whether `message` is the response to the request `id`.
fn is_answer(message: &Json, id: Option<&Json>) -> bool {
    id.is_some_and(|id| message.get("id") == Some(id))
        && message.get("method").is_none()
        && (message.get("result").is_some() || message.get("error").is_some())
}

/// A JSON-RPC response or error in `body`.
fn json_rpc_answer(body: &str) -> Option<Json> {
    let message: Json = serde_json::from_str(body).ok()?;
    (message.get("jsonrpc").is_some() && (message.get("result").is_some() || message.get("error").is_some()))
        .then_some(message)
}

/// `text` as JSON, and as one line to write: itself, unless it spans lines.
fn json_line(text: &str) -> Option<(String, Json)> {
    let message: Json = serde_json::from_str(text).ok()?;
    let text = text.trim();
    let line = if text.contains(['\n', '\r']) { message.to_string() } else { text.to_owned() };
    Some((line, message))
}

/// The protocol version an `initialize` response settled on.
fn negotiated_version(answer: &Json) -> Option<String> {
    answer.get("result")?.get("protocolVersion")?.as_str().map(str::to_owned)
}

fn error_message(id: &Json, code: i32, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// A SEP-2243 header value: as it is when it can travel as one, else
/// Base64 inside `=?base64?…?=`.
fn standard_header_value(text: &str) -> Option<HeaderValue> {
    let sentinel = text.starts_with("=?base64?") && text.ends_with("?=");
    let bare = text.chars().all(|c| matches!(c, ' '..='~')) && !text.starts_with(' ') && !text.ends_with(' ');
    let value =
        if bare && !sentinel { text.to_owned() } else { format!("=?base64?{}?=", BASE64_STANDARD.encode(text)) };
    HeaderValue::from_str(&value).ok()
}

/// An error and its causes: reqwest's own message leaves them out.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn log(message: &str) {
    eprintln!("idedb mcp-bridge: {message}");
}

/// Splits an SSE stream into the data of its events, as WHATWG's event
/// stream format says: lines end in LF, CRLF or CR; `data` lines add to the
/// event; a blank line ends it; comments, `id`, `retry` and `event` carry
/// nothing to relay. An event with empty data (rmcp's priming event) holds
/// no message, and is skipped.
#[derive(Debug, Default)]
struct SseParser {
    /// Bytes of a line not ended yet.
    pending: Vec<u8>,
    data: String,
}

impl SseParser {
    /// The data of every event `bytes` complete.
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|&b| b == b'\n' || b == b'\r') {
            let end = start + offset;
            let mut next = end + 1;
            if self.pending[end] == b'\r' {
                match self.pending.get(next) {
                    // The LF of a CRLF may come in the next chunk.
                    None => break,
                    Some(b'\n') => next += 1,
                    Some(_) => {}
                }
            }
            let line = String::from_utf8_lossy(&self.pending[start..end]).into_owned();
            self.line(&line, &mut events);
            start = next;
        }
        self.pending.drain(..start);
        events
    }

    fn line(&mut self, line: &str, events: &mut Vec<String>) {
        if line.is_empty() {
            if let Some(data) = self.data.strip_suffix('\n')
                && !data.is_empty()
            {
                events.push(data.to_owned());
            }
            self.data.clear();
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            self.data.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], vars: &[(&str, &str)]) -> Result<Invocation, String> {
        let args = args.iter().map(|arg| arg.to_string());
        parse_args(args, |name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string()))
    }

    fn run(port: u16, token: Option<&str>) -> Invocation {
        Invocation::Run { port, token: token.map(str::to_owned) }
    }

    #[test]
    fn reads_the_port_from_the_arguments_then_the_environment() {
        assert_eq!(parse(&[], &[]), Ok(run(7412, None)));
        assert_eq!(parse(&["--port", "7500"], &[]), Ok(run(7500, None)));
        assert_eq!(parse(&["--port=7501"], &[]), Ok(run(7501, None)));
        assert_eq!(parse(&[], &[(PORT_VAR, "7600")]), Ok(run(7600, None)));
        assert_eq!(parse(&[], &[(PORT_VAR, " ")]), Ok(run(7412, None)));
        assert_eq!(parse(&["--port", "7500"], &[(PORT_VAR, "7600")]), Ok(run(7500, None)));
        for bad in ["0", "65536", "port", "-1", ""] {
            let error = parse(&["--port", bad], &[]).unwrap_err();
            assert!(error.starts_with("--port: "), "{error}");
        }
        assert_eq!(parse(&["--port"], &[]), Err("--port needs a port number".into()));
        let error = parse(&[], &[(PORT_VAR, "x")]).unwrap_err();
        assert!(error.starts_with("IDEDB_MCP_PORT: `x` is not a port"), "{error}");
    }

    #[test]
    fn reads_the_token_from_the_environment_only() {
        assert_eq!(parse(&[], &[(TOKEN_VAR, " idedb_abc\n")]), Ok(run(7412, Some("idedb_abc"))));
        // Missing, or empty as env blocks leave it: the bridge still runs,
        // and answers each request with how to set it.
        assert_eq!(parse(&[], &[(TOKEN_VAR, "")]), Ok(run(7412, None)));
        let error = parse(&["--token", "idedb_abc"], &[]).unwrap_err();
        assert!(error.contains(TOKEN_VAR), "{error}");
        assert!(parse(&["--token=idedb_abc"], &[]).unwrap_err().contains(TOKEN_VAR));
    }

    #[test]
    fn help_and_unknown_arguments() {
        assert_eq!(parse(&["--help"], &[]), Ok(Invocation::Help));
        assert_eq!(parse(&["--port", "7500", "-h"], &[]), Ok(Invocation::Help));
        assert_eq!(parse(&["serve"], &[]), Err("unexpected argument `serve`".into()));
        assert!(USAGE.contains("--port") && USAGE.contains(TOKEN_VAR) && USAGE.contains(PORT_VAR));
    }

    #[test]
    fn splits_sse_into_event_data_across_chunks() {
        let mut parser = SseParser::default();
        // A priming event (no data), a keep-alive comment, then two
        // messages split at awkward places, with CRLF and CR line ends.
        assert!(parser.feed(b"id: 0\nretry: 3000\ndata:\n\n: keep-alive\n\nda").is_empty());
        assert!(parser.feed(b"ta: {\"id\":1}\r").is_empty());
        assert_eq!(parser.feed(b"\n\r\ndata:{\"a\":\r"), ["{\"id\":1}"]);
        // Multi-line data joins with LF. The last CR may still be a CRLF.
        assert!(parser.feed(b"data: 2}\r\r").is_empty());
        assert_eq!(parser.feed(b"event: message\ndata: x"), ["{\"a\":\n2}"]);
        assert_eq!(parser.feed(b"\n\n"), ["x"]);
    }

    #[test]
    fn reads_what_routing_needs_from_a_message() {
        let call = r#"{"jsonrpc":"2.0","id":"a-1","method":"tools/call","params":{"name":"query","arguments":{},
            "_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","progressToken":7}}}"#;
        let message = Message::parse(call.into()).unwrap();
        assert_eq!(message.id, Some(json!("a-1")));
        assert_eq!((message.method.as_deref(), message.name.as_deref()), (Some("tools/call"), Some("query")));
        assert_eq!(message.version.as_deref(), Some("2026-07-28"));
        assert!(!message.is_barrier());

        let read = r#"{"jsonrpc":"2.0","id":0,"method":"resources/read","params":{"uri":"file:///x"}}"#;
        let read = Message::parse(read.into()).unwrap();
        assert_eq!((read.name.as_deref(), read.version), (Some("file:///x"), None));

        let cancelled = r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}"#;
        let cancelled = Message::parse(cancelled.into()).unwrap();
        assert_eq!((cancelled.id, cancelled.cancels), (None, Some(json!(5))));

        let initialize = Message::parse(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#.into());
        assert!(initialize.unwrap().is_barrier());
        assert!(Message::parse(INITIALIZED.into()).unwrap().is_barrier());
        // A response to a server's request has an id, but is no request.
        let response = Message::parse(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#.into()).unwrap();
        assert_eq!((response.id, response.method), (None, None));

        assert_eq!(Message::parse("{".into()).unwrap_err().0, PARSE_ERROR);
        assert_eq!(Message::parse("[]".into()).unwrap_err().0, INVALID_REQUEST);
    }

    #[test]
    fn standard_header_values_wrap_what_cant_travel_bare() {
        let value = |text: &str| standard_header_value(text).unwrap().to_str().unwrap().to_owned();
        assert_eq!(value("list_tables"), "list_tables");
        assert_eq!(value("a b"), "a b");
        assert_eq!(value("café"), format!("=?base64?{}?=", BASE64_STANDARD.encode("café")));
        assert_eq!(value(" padded"), format!("=?base64?{}?=", BASE64_STANDARD.encode(" padded")));
        assert_eq!(value("=?base64?eA==?="), format!("=?base64?{}?=", BASE64_STANDARD.encode("=?base64?eA==?=")));
    }

    #[test]
    fn failures_say_what_to_do() {
        assert!(Failure::NoToken.message(7412).contains("IdeDB → MCP → Clients"));
        assert!(Failure::Unauthorized.message(7412).starts_with("IdeDB rejected the client token"));
        assert_eq!(
            Failure::NotRunning.message(7500),
            "IdeDB isn't running or its MCP server is off. Open IdeDB → MCP → Server and turn it on (port 7500)."
        );
        assert_eq!((Failure::NoToken.code(), Failure::NotRunning.code()), (-32001, -32000));
        let large = Failure::Status { status: StatusCode::PAYLOAD_TOO_LARGE, body: String::new() };
        assert!(large.message(7412).contains("1 MiB"));
        let long = Failure::Status { status: StatusCode::BAD_GATEWAY, body: "é".repeat(300) };
        assert_eq!(long.message(7412), format!("IdeDB answered HTTP 502 Bad Gateway: {}….", "é".repeat(200)));
    }
}
