//! The stdio bridge end to end: `bridge::relay` over in-memory pipes, as a
//! stdio MCP client (Claude Desktop) would drive it, against the server on a
//! free port with SQLite files for data sources.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use idedb_mcp::bridge::{BridgeConfig, Launcher, relay};
use idedb_mcp::testing::{TestHost, exec};
use idedb_mcp::{ApprovalRequest, AuditEntry, Decision, MCP_PATH, McpEvent, McpServer, McpSettings, Transport};
use idedb_store::{Access, DataSource};
use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

/// How the client starts talking to IdeDB.
#[derive(Debug, Clone, Copy)]
enum Lifecycle {
    /// 2026-07-28: no `initialize`; each request carries its protocol
    /// version, client info and capabilities in `_meta`.
    Modern,
    /// `initialize` at 2025-06-18, then a session.
    Legacy,
}

const LIFECYCLES: [Lifecycle; 2] = [Lifecycle::Modern, Lifecycle::Legacy];

impl Lifecycle {
    /// Opens the conversation, as the client does before anything else.
    async fn open(self, stdio: &mut Stdio) {
        if let Lifecycle::Legacy = self {
            stdio.send(initialize(json!(0))).await;
            let answer = stdio.recv().await;
            assert_eq!(answer["result"]["protocolVersion"], "2025-06-18", "{answer}");
            stdio.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        }
    }

    fn version(self) -> &'static str {
        match self {
            Lifecycle::Modern => "2026-07-28",
            Lifecycle::Legacy => "2025-06-18",
        }
    }

    /// A request as this lifecycle's client sends it.
    fn request(self, id: impl Into<Json>, method: &str, mut params: Json) -> Json {
        if let Lifecycle::Modern = self {
            let meta = params["_meta"].as_object().cloned().unwrap_or_default();
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": { "name": "claude-ai", "version": "0.2.0" },
                "io.modelcontextprotocol/clientCapabilities": {},
            });
            params["_meta"].as_object_mut().unwrap().extend(meta);
        }
        json!({ "jsonrpc": "2.0", "id": id.into(), "method": method, "params": params })
    }

    fn call(self, id: impl Into<Json>, tool: &str, arguments: Json) -> Json {
        self.request(id, "tools/call", json!({ "name": tool, "arguments": arguments }))
    }

    /// A call that asks for progress reports.
    fn call_with_progress(self, id: impl Into<Json>, tool: &str, arguments: Json, token: &str) -> Json {
        let params = json!({ "name": tool, "arguments": arguments, "_meta": { "progressToken": token } });
        self.request(id, "tools/call", params)
    }
}

fn initialize(id: Json) -> Json {
    let client = json!({ "name": "claude-ai", "version": "0.1.0" });
    let params = json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": client });
    json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": params })
}

fn cancelled(id: u64) -> Json {
    json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id, "reason": "user" } })
}

/// The bridge's stdin and stdout, from the client's side.
struct Stdio {
    stdin: DuplexStream,
    stdout: Lines<BufReader<DuplexStream>>,
    relay: JoinHandle<std::io::Result<()>>,
}

impl Stdio {
    fn start(config: BridgeConfig, launcher: impl Launcher) -> Self {
        let (stdin, bridge_in) = tokio::io::duplex(1 << 16);
        let (bridge_out, stdout) = tokio::io::duplex(1 << 16);
        let relay = tokio::spawn(relay(bridge_in, bridge_out, config, launcher));
        Self { stdin, stdout: BufReader::new(stdout).lines(), relay }
    }

    async fn send(&mut self, message: Json) {
        self.send_line(&message.to_string()).await;
    }

    async fn send_line(&mut self, line: &str) {
        self.stdin.write_all(format!("{line}\n").as_bytes()).await.unwrap();
    }

    /// The next line the bridge writes, within 10 s.
    async fn recv(&mut self) -> Json {
        let line = timeout(Duration::from_secs(10), self.stdout.next_line()).await.expect("a line in time");
        let line = line.unwrap().expect("a line before the end");
        serde_json::from_str(&line).unwrap()
    }

    /// The lines until the response to `id`, that one last.
    async fn recv_until(&mut self, id: u64) -> Vec<Json> {
        let mut lines = Vec::new();
        loop {
            let line = self.recv().await;
            let done = line["id"] == id;
            lines.push(line);
            if done {
                return lines;
            }
        }
    }

    /// Closes stdin, as a client does when it is done, and waits for the
    /// bridge to return.
    async fn close(self) {
        let Self { stdin, mut stdout, relay } = self;
        drop(stdin);
        // Whatever it still writes, until it closes stdout.
        let drained = async { while let Ok(Some(_)) = stdout.next_line().await {} };
        timeout(Duration::from_secs(10), drained).await.expect("stdout closed in time");
        timeout(Duration::from_secs(10), relay).await.expect("the bridge returned in time").unwrap().unwrap();
    }
}

struct Env {
    test: Arc<TestHost>,
    server: McpServer,
    addr: SocketAddr,
    dir: tempfile::TempDir,
}

impl Env {
    async fn start() -> Self {
        let env = Self::stopped();
        let addr = env.server.start(&on_port(0)).await.unwrap();
        Self { addr, ..env }
    }

    /// A server that doesn't listen yet, on a port nothing holds now.
    fn stopped() -> Self {
        let test = TestHost::new();
        // Often enough for a test to see a write's progress reports.
        let server = McpServer::with_progress_every(test.clone(), Duration::from_millis(100));
        let addr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        Self { test, server, addr, dir: tempfile::tempdir().unwrap() }
    }

    /// A SQLite data source with `t (id, name)` holding three rows.
    async fn source(&self, name: &str) -> DataSource {
        let source = self.test.save_sqlite(name, &self.dir.path().join(format!("{name}.db")));
        exec(&source, None, "create table t (id integer primary key, name text not null)").await;
        exec(&source, None, "insert into t (name) values ('a'), ('b'), ('c')").await;
        source
    }

    fn config(&self, token: Option<&str>) -> BridgeConfig {
        BridgeConfig::new(self.addr.port(), token.map(str::to_owned))
    }

    /// The bridge with `token`, never needing to open IdeDB.
    fn bridge(&self, token: &str) -> Stdio {
        Stdio::start(self.config(Some(token)), never_launched())
    }
}

fn on_port(port: u16) -> McpSettings {
    McpSettings { port, ..McpSettings::default() }
}

/// A launcher for a server that is already up.
fn never_launched() -> impl Launcher {
    || async { unexpected_launch() }
}

fn unexpected_launch() -> Result<(), String> {
    panic!("the bridge tried to open IdeDB")
}

/// A launcher that counts how often it is asked, and runs `then` each time.
fn counting(count: Arc<AtomicUsize>, then: impl Fn() + Send + Sync + 'static) -> impl Launcher {
    move || {
        count.fetch_add(1, Ordering::SeqCst);
        then();
        async { Ok::<(), String>(()) }
    }
}

async fn names(source: &DataSource) -> Vec<String> {
    let rows = exec(source, None, "select name from t order by id").await;
    rows.into_iter().map(|row| format!("{:?}", row[0])).collect()
}

/// The next event `pick` accepts, within 10 s.
async fn next<T>(events: &mut broadcast::Receiver<McpEvent>, pick: impl Fn(McpEvent) -> Option<T>) -> T {
    let found = async {
        loop {
            if let Some(found) = pick(events.recv().await.unwrap()) {
                return found;
            }
        }
    };
    timeout(Duration::from_secs(10), found).await.expect("the event in time")
}

async fn next_approval(events: &mut broadcast::Receiver<McpEvent>) -> ApprovalRequest {
    next(events, |event| match event {
        McpEvent::ApprovalRequested(request) => Some(request),
        _ => None,
    })
    .await
}

async fn next_audit(events: &mut broadcast::Receiver<McpEvent>) -> AuditEntry {
    next(events, |event| match event {
        McpEvent::Audit(entry) => Some(entry),
        _ => None,
    })
    .await
}

/// Asserts a JSON-RPC error for `id` with `code`, and returns its message.
fn error_of(message: &Json, id: impl Into<Json>, code: i64) -> String {
    assert_eq!(message["id"], id.into(), "{message}");
    assert_eq!(message["error"]["code"], code, "{message}");
    message["error"]["message"].as_str().unwrap().to_owned()
}

const SELECT: &str = "select * from t order by id";

#[tokio::test]
async fn relays_a_session_from_initialize_to_its_calls() {
    let env = Env::start().await;
    let shop = env.source("shop").await;
    let (caller, token) = env.test.client("claude-desktop", &[(&shop, Access::Read)]);
    let config = env.config(Some(&token));
    let instance = config.instance.clone();
    let mut stdio = Stdio::start(config, never_launched());

    // String ids come back as they went.
    stdio.send(initialize(json!("init"))).await;
    let answer = stdio.recv().await;
    assert_eq!(answer["id"], "init");
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "IdeDB");
    // Nothing comes back for a notification: the next line answers the
    // next request.
    stdio.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;

    stdio.send(Lifecycle::Legacy.request(1, "tools/list", json!({}))).await;
    let listed = stdio.recv().await;
    assert_eq!(listed["id"], 1);
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6, "{listed}");
    // The bridge sends no Mcp-Param-* headers: no tool may ask for them.
    assert!(!listed.to_string().contains("x-mcp-header"), "{listed}");

    stdio.send(Lifecycle::Legacy.call(2, "query", json!({ "connection": "shop", "sql": SELECT }))).await;
    let answer = stdio.recv().await;
    assert_eq!(answer["id"], 2);
    assert_eq!(answer["result"]["structuredContent"]["rows"], json!([[1, "a"], [2, "b"], [3, "c"]]), "{answer}");

    let audit = env.test.last_audit();
    assert_eq!((audit.tool.as_str(), audit.decision), ("query", Decision::Allowed));
    assert_eq!(audit.client_id.as_deref(), Some(caller.client_id.as_str()));
    assert_eq!(audit.transport, Transport::Bridge);
    assert_eq!(audit.protocol_version.as_deref(), Some("2025-06-18"));
    assert_eq!(audit.client_info_name.as_deref(), Some("claude-ai"));
    // The session's id (a UUID from rmcp), not the bridge's.
    let session = audit.session_key.unwrap();
    assert_eq!(session.len(), 36);
    assert_ne!(session, instance);

    // A line that is no JSON gets a parse error, and the bridge goes on.
    stdio.send_line("{ not json").await;
    assert!(error_of(&stdio.recv().await, Json::Null, -32700).starts_with("Parse error"));
    stdio.send(Lifecycle::Legacy.call(3, "list_connections", json!({}))).await;
    assert_eq!(stdio.recv().await["result"]["structuredContent"]["connections"][0]["name"], "shop");
    stdio.close().await;
}

#[tokio::test]
async fn relays_2026_07_28_calls_without_a_session() {
    let env = Env::start().await;
    let shop = env.source("shop").await;
    let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Read)]);
    let config = env.config(Some(&token));
    let instance = config.instance.clone();
    let mut stdio = Stdio::start(config, never_launched());

    let modern = Lifecycle::Modern;
    stdio.send(modern.request(1, "tools/list", json!({}))).await;
    assert_eq!(stdio.recv().await["result"]["tools"].as_array().unwrap().len(), 6);
    stdio.send(modern.call(2, "query", json!({ "connection": "shop", "sql": "select count(*) as n from t" }))).await;
    let answer = stdio.recv().await;
    assert_eq!((answer["id"].clone(), answer["result"]["structuredContent"]["rows"].clone()), (json!(2), json!([[3]])));

    let audit = env.test.last_audit();
    assert_eq!(audit.transport, Transport::Bridge);
    // Outside a session, the bridge's own id.
    assert_eq!(audit.session_key.as_deref(), Some(instance.as_str()));
    assert_eq!(audit.protocol_version.as_deref(), Some("2026-07-28"));
    assert_eq!(
        (audit.client_info_name.as_deref(), audit.client_info_version.as_deref()),
        (Some("claude-ai"), Some("0.2.0"))
    );

    // What rmcp refuses comes back as its own error for the request: here,
    // a body over 1 MiB.
    stdio.send(modern.call(3, "query", json!({ "connection": "shop", "sql": "x".repeat(1024 * 1024) }))).await;
    assert!(error_of(&stdio.recv().await, 3, -32000).contains("1 MiB"));
    stdio.close().await;
}

#[tokio::test]
async fn a_write_reports_progress_until_the_user_approves_it() {
    for lifecycle in LIFECYCLES {
        let env = Env::start().await;
        let shop = env.source("shop").await;
        let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Write)]);
        let mut stdio = env.bridge(&token);
        lifecycle.open(&mut stdio).await;

        // The user takes a moment: long enough for a few progress reports.
        let mut events = env.test.subscribe();
        let server = env.server.clone();
        let user = tokio::spawn(async move {
            let request = next_approval(&mut events).await;
            tokio::time::sleep(Duration::from_millis(450)).await;
            assert!(server.answer_approval(request.id, true));
            request
        });
        let update = json!({ "connection": "shop", "sql": "update t set name = 'z'", "reason": "a test" });
        stdio.send(lifecycle.call_with_progress(3, "execute", update, "p-1")).await;
        let lines = stdio.recv_until(3).await;
        let request = user.await.unwrap();
        assert_eq!(request.client_info.map(|info| info.name).as_deref(), Some("claude-ai"));

        let (result, progress) = lines.split_last().unwrap();
        assert!(progress.len() >= 2, "{lifecycle:?}: {lines:?}");
        for report in progress {
            assert_eq!(report["method"], "notifications/progress", "{report}");
            assert_eq!(report["params"]["progressToken"], "p-1", "{report}");
        }
        let message = progress[0]["params"]["message"].as_str().unwrap();
        assert!(message.starts_with("Waiting for the user to approve the statement"), "{message}");
        assert_eq!(result["result"]["isError"], false, "{result}");
        assert_eq!(result["result"]["structuredContent"]["rowCount"], 3, "{result}");
        assert_eq!(names(&shop).await, ["Text(\"z\")"; 3]);
        let audit = env.test.last_audit();
        assert_eq!((audit.decision, audit.transport), (Decision::Approved, Transport::Bridge));
        assert_eq!(audit.protocol_version.as_deref(), Some(lifecycle.version()));
        stdio.close().await;
    }
}

#[tokio::test]
async fn reads_go_on_while_a_write_waits_for_approval() {
    for lifecycle in LIFECYCLES {
        let env = Env::start().await;
        let shop = env.source("shop").await;
        let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Write)]);
        let mut stdio = env.bridge(&token);
        lifecycle.open(&mut stdio).await;

        let mut events = env.test.subscribe();
        stdio.send(lifecycle.call(10, "execute", json!({ "connection": "shop", "sql": "delete from t" }))).await;
        let request = next_approval(&mut events).await;

        stdio.send(lifecycle.call(11, "query", json!({ "connection": "shop", "sql": SELECT }))).await;
        let read = stdio.recv().await;
        assert_eq!(read["id"], 11, "{lifecycle:?}: {read}");
        assert_eq!(read["result"]["structuredContent"]["rowCount"], 3, "{read}");
        assert_eq!(env.server.pending_approvals().len(), 1);

        assert!(env.server.answer_approval(request.id, true));
        let write = stdio.recv().await;
        assert_eq!(write["id"], 10, "{write}");
        assert_eq!(write["result"]["structuredContent"]["rowCount"], 3, "{write}");
        assert!(names(&shop).await.is_empty());
        stdio.close().await;
    }
}

#[tokio::test]
async fn cancelling_a_call_withdraws_its_approval() {
    for lifecycle in LIFECYCLES {
        let env = Env::start().await;
        let shop = env.source("shop").await;
        let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Write)]);
        let mut stdio = env.bridge(&token);
        lifecycle.open(&mut stdio).await;

        let mut events = env.test.subscribe();
        stdio.send(lifecycle.call(5, "execute", json!({ "connection": "shop", "sql": "delete from t" }))).await;
        next_approval(&mut events).await;
        stdio.send(cancelled(5)).await;

        let audit = next_audit(&mut events).await;
        assert_eq!(audit.decision, Decision::Withdrawn, "{lifecycle:?}: {audit:?}");
        assert!(audit.error.unwrap().starts_with("The call was cancelled before the user answered"));
        assert!(env.server.pending_approvals().is_empty());
        assert_eq!(names(&shop).await.len(), 3);
        // Nothing for the cancelled call: the next line answers the next
        // request.
        stdio.send(lifecycle.call(6, "list_connections", json!({}))).await;
        assert_eq!(stdio.recv().await["id"], 6);
        stdio.close().await;
    }
}

#[tokio::test]
async fn a_rejected_token_is_an_error_for_the_request() {
    let env = Env::start().await;
    let shop = env.source("shop").await;

    let mut stdio = env.bridge("idedb_wrong");
    stdio.send(initialize(json!("init-1"))).await;
    let message = error_of(&stdio.recv().await, "init-1", -32001);
    assert!(message.contains("wrong or was revoked") && message.contains("IdeDB → MCP → Clients"), "{message}");
    stdio.send(Lifecycle::Modern.call(7, "list_connections", json!({}))).await;
    error_of(&stdio.recv().await, 7, -32001);
    stdio.close().await;

    // Revoked mid-session.
    let (caller, token) = env.test.client("claude-desktop", &[(&shop, Access::Read)]);
    let mut stdio = env.bridge(&token);
    Lifecycle::Legacy.open(&mut stdio).await;
    env.test.store.mcp_client_revoke(&caller.client_id).unwrap();
    stdio.send(Lifecycle::Legacy.call(8, "list_connections", json!({}))).await;
    assert!(error_of(&stdio.recv().await, 8, -32001).starts_with("IdeDB rejected the client token"));
    stdio.close().await;
}

#[tokio::test]
async fn without_a_token_every_request_says_how_to_set_it() {
    let env = Env::stopped();
    let launches = Arc::new(AtomicUsize::new(0));
    let mut stdio = Stdio::start(env.config(None), counting(launches.clone(), || {}));

    stdio.send(initialize(json!(0))).await;
    let message = error_of(&stdio.recv().await, 0, -32001);
    assert!(message.starts_with("IDEDB_MCP_TOKEN is not set") && message.contains("IdeDB → MCP → Clients"));
    stdio.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    stdio.send(Lifecycle::Modern.request(1, "tools/list", json!({}))).await;
    error_of(&stdio.recv().await, 1, -32001);
    stdio.close().await;
    // Without a token, IdeDB isn't even asked.
    assert_eq!(launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn opens_idedb_when_nothing_listens() {
    let env = Env::stopped();
    let shop = env.source("shop").await;
    let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Read)]);
    let launches = Arc::new(AtomicUsize::new(0));
    // As `open` does: the app starts listening a moment later.
    let (server, port) = (env.server.clone(), env.addr.port());
    let launcher = counting(launches.clone(), move || {
        let server = server.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            server.start(&on_port(port)).await.unwrap();
        });
    });
    let mut stdio = Stdio::start(env.config(Some(&token)), launcher);

    // Two calls at once, while nothing listens: IdeDB is opened once.
    stdio.send(Lifecycle::Modern.request(1, "tools/list", json!({}))).await;
    stdio.send(Lifecycle::Modern.call(2, "query", json!({ "connection": "shop", "sql": SELECT }))).await;
    let mut answers = [stdio.recv().await, stdio.recv().await];
    answers.sort_by_key(|answer| answer["id"].as_u64());
    assert_eq!(answers[0]["result"]["tools"].as_array().unwrap().len(), 6, "{}", answers[0]);
    assert_eq!(answers[1]["result"]["structuredContent"]["rowCount"], 3, "{}", answers[1]);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    stdio.close().await;
}

#[tokio::test]
async fn gives_up_when_idedb_never_listens() {
    let env = Env::stopped();
    let port = env.addr.port();
    let not_running = format!(
        "IdeDB isn't running or its MCP server is off. Open IdeDB → MCP → Server and turn it on (port {port})."
    );
    let launches = Arc::new(AtomicUsize::new(0));
    let config = BridgeConfig {
        launch_wait: Duration::from_millis(400),
        retry_every: Duration::from_millis(50),
        ..env.config(Some("idedb_token"))
    };
    let mut stdio = Stdio::start(config, counting(launches.clone(), || {}));

    let started = Instant::now();
    stdio.send(Lifecycle::Modern.request(1, "tools/list", json!({}))).await;
    stdio.send(Lifecycle::Modern.request(2, "tools/list", json!({}))).await;
    let mut errors = [stdio.recv().await, stdio.recv().await];
    errors.sort_by_key(|error| error["id"].as_u64());
    assert_eq!(error_of(&errors[0], 1, -32000), not_running);
    assert_eq!(error_of(&errors[1], 2, -32000), not_running);
    assert!(started.elapsed() >= Duration::from_millis(400));
    // The wait is over: the next request fails at once, and IdeDB isn't
    // opened again.
    let started = Instant::now();
    stdio.send(initialize(json!(3))).await;
    assert_eq!(error_of(&stdio.recv().await, 3, -32000), not_running);
    assert!(started.elapsed() < Duration::from_millis(300));
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    stdio.close().await;

    // When IdeDB can't be opened at all (not on macOS, or not installed),
    // there is nothing to wait for.
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let failing = move || {
        count.fetch_add(1, Ordering::SeqCst);
        async { Err("no IdeDB here".to_owned()) }
    };
    let mut stdio = Stdio::start(env.config(Some("idedb_token")), failing);
    let started = Instant::now();
    stdio.send(Lifecycle::Modern.request(1, "tools/list", json!({}))).await;
    assert_eq!(error_of(&stdio.recv().await, 1, -32000), not_running);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    stdio.close().await;
}

#[tokio::test]
async fn ending_stdin_ends_the_session_and_its_pending_writes() {
    for lifecycle in LIFECYCLES {
        let env = Env::start().await;
        let shop = env.source("shop").await;
        let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Write)]);
        let config = BridgeConfig { shutdown_grace: Duration::from_millis(200), ..env.config(Some(&token)) };
        let mut stdio = Stdio::start(config, never_launched());
        lifecycle.open(&mut stdio).await;
        stdio.send(lifecycle.call(1, "query", json!({ "connection": "shop", "sql": SELECT }))).await;
        assert_eq!(stdio.recv().await["id"], 1);
        let session = env.test.last_audit().session_key.unwrap();

        let mut events = env.test.subscribe();
        stdio.send(lifecycle.call(2, "execute", json!({ "connection": "shop", "sql": "delete from t" }))).await;
        next_approval(&mut events).await;
        let closed = Instant::now();
        stdio.close().await;

        // Withdrawn right after the grace period, without the 5 s rmcp gives
        // the calls of a session closed under them.
        assert_eq!(next_audit(&mut events).await.decision, Decision::Withdrawn, "{lifecycle:?}");
        assert!(closed.elapsed() < Duration::from_secs(2), "{lifecycle:?}: {:?}", closed.elapsed());
        assert!(env.server.pending_approvals().is_empty());
        assert_eq!(names(&shop).await.len(), 3);
        if let Lifecycle::Legacy = lifecycle {
            // The session is gone.
            let listed = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }).to_string();
            let response = reqwest::Client::new()
                .post(format!("http://{}{MCP_PATH}", env.addr))
                .bearer_auth(&token)
                .header("accept", "application/json, text/event-stream")
                .header("content-type", "application/json")
                .header("mcp-protocol-version", "2025-06-18")
                .header("mcp-session-id", &session)
                .body(listed);
            assert_eq!(response.send().await.unwrap().status(), 404);
        }
    }
}

#[tokio::test]
async fn a_restarted_server_gets_a_new_session() {
    let env = Env::start().await;
    let shop = env.source("shop").await;
    let (_, token) = env.test.client("claude-desktop", &[(&shop, Access::Read)]);
    let mut stdio = env.bridge(&token);
    Lifecycle::Legacy.open(&mut stdio).await;
    stdio.send(Lifecycle::Legacy.call(1, "list_connections", json!({}))).await;
    assert_eq!(stdio.recv().await["id"], 1);
    let before = env.test.last_audit().session_key.unwrap();

    // The user restarts the server (or the app): its sessions are gone.
    env.server.stop().await;
    env.server.start(&on_port(env.addr.port())).await.unwrap();
    stdio.send(Lifecycle::Legacy.call(2, "query", json!({ "connection": "shop", "sql": SELECT }))).await;
    let answer = stdio.recv().await;
    assert_eq!(answer["result"]["structuredContent"]["rowCount"], 3, "{answer}");
    let audit = env.test.last_audit();
    let after = audit.session_key.unwrap();
    assert_ne!(after, before);
    assert_eq!(after.len(), 36);
    assert_eq!((audit.transport, audit.client_info_name.as_deref()), (Transport::Bridge, Some("claude-ai")));
    stdio.close().await;
}
