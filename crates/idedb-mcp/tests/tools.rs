//! The tools end to end on SQLite files: grants, classification, approvals,
//! results and the audit rows they leave.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use idedb_core::Engine;
use idedb_mcp::testing::{TestHost, exec};
use idedb_mcp::{
    AccessLevel, ApprovalRequest, CallError, Caller, CancellationToken, ClientInfo, Decision, DescribeTableArgs,
    ExecuteArgs, ExecuteOutput, ListSchemasArgs, ListTablesArgs, McpEvent, McpServer, McpSettings, Progress,
    QueryArgs, QueryOutput, TableKind, Transport,
};
use idedb_sql::{Kind, WriteKind};
use idedb_store::{Access, DataSource};
use serde_json::json;

struct Env {
    test: Arc<TestHost>,
    server: McpServer,
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let test = TestHost::new();
        let server = McpServer::new(test.clone());
        Self { test, server, dir: tempfile::tempdir().unwrap() }
    }

    /// A SQLite data source with `t (id, name)` holding three rows, and
    /// `u` referencing it.
    async fn source(&self, name: &str) -> DataSource {
        let source = self.test.save_sqlite(name, &self.dir.path().join(format!("{name}.db")));
        exec(&source, None, "create table t (id integer primary key, name text not null)").await;
        exec(&source, None, "insert into t (name) values ('a'), ('b'), ('c')").await;
        exec(&source, None, "create table u (id integer primary key, t_id integer references t (id))").await;
        source
    }

    fn settings(&self, change: impl FnOnce(&mut McpSettings)) {
        let mut settings = self.server.settings().unwrap();
        change(&mut settings);
        self.server.save_settings(&settings).unwrap();
    }

    async fn query(&self, caller: &Caller, connection: &str, sql: &str) -> Result<QueryOutput, CallError> {
        let args = QueryArgs { connection: connection.into(), sql: sql.into(), schema: None, max_rows: None };
        self.server.query(caller, args, &CancellationToken::new()).await
    }

    async fn execute(&self, caller: &Caller, connection: &str, sql: &str) -> Result<ExecuteOutput, CallError> {
        let args = ExecuteArgs { connection: connection.into(), sql: sql.into(), schema: None, reason: None };
        self.server.execute(caller, args, |_| {}, &CancellationToken::new()).await
    }

    /// Answers the next approval request with `approve`, returning it.
    fn answer_next(&self, approve: bool) -> tokio::task::JoinHandle<ApprovalRequest> {
        let mut events = self.test.subscribe();
        let server = self.server.clone();
        tokio::spawn(async move {
            loop {
                if let McpEvent::ApprovalRequested(request) = events.recv().await.unwrap() {
                    assert!(server.answer_approval(request.id, approve));
                    return request;
                }
            }
        })
    }

    fn approvals_requested(&self) -> usize {
        self.test.events().iter().filter(|e| matches!(e, McpEvent::ApprovalRequested(_))).count()
    }
}

/// The message of a tool error; panics on anything else.
fn tool_error<T: std::fmt::Debug>(result: Result<T, CallError>) -> String {
    match result {
        Err(CallError::Tool(e)) => e.message,
        other => panic!("expected a tool error, got {other:?}"),
    }
}

async fn names(source: &DataSource) -> Vec<String> {
    let rows = exec(source, None, "select name from t order by id").await;
    rows.into_iter().map(|row| format!("{:?}", row[0])).collect()
}

#[tokio::test]
async fn lists_only_granted_connections() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let hidden = env.source("hidden").await;
    let no_password = env.test.save(DataSource {
        id: String::new(),
        name: "pg".into(),
        params: idedb_core::ConnectionParams {
            engine: Engine::Postgres,
            host: "localhost".into(),
            port: None,
            user: "u".into(),
            database: "app".into(),
            ssl_mode: idedb_core::SslMode::Prefer,
            path: String::new(),
        },
        color: None,
        save_password: false,
    });
    let mut uri = env.test.save_sqlite("uri", &env.dir.path().join("uri.db"));
    uri.params.path = format!("file:{}?mode=ro", uri.params.path);
    let uri = env.test.save(uri);
    let (caller, _) =
        env.test.client("claude", &[(&shop, Access::Write), (&no_password, Access::Read), (&uri, Access::Read)]);

    let listed = env.server.list_connections(&caller).await.unwrap().connections;
    let ids: Vec<&str> = listed.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, [shop.id.as_str(), no_password.id.as_str(), uri.id.as_str()]);
    assert!(!ids.contains(&hidden.id.as_str()));

    let json = serde_json::to_value(&listed[0]).unwrap();
    assert_eq!(
        json,
        json!({
            "id": shop.id, "name": "shop", "engine": "sqlite", "database": "shop.db",
            "access": "write", "writesAllowed": true, "available": true,
        })
    );
    assert_eq!((listed[1].access, listed[1].available), (AccessLevel::Read, false));
    assert_eq!(listed[1].database.as_deref(), Some("app"));
    assert!(listed[1].note.as_deref().unwrap().contains("password isn't saved"), "{:?}", listed[1].note);
    assert!(!listed[2].available);
    assert!(listed[2].note.as_deref().unwrap().contains("file: URI"), "{:?}", listed[2].note);

    // Never-write turns writes off whatever the grant.
    env.test.store.mcp_set_never_write(&shop.id, true).unwrap();
    let listed = env.server.list_connections(&caller).await.unwrap().connections;
    assert!(!listed[0].writes_allowed);

    let audit = env.test.last_audit();
    assert_eq!((audit.tool.as_str(), audit.decision), ("list_connections", Decision::Allowed));
    assert_eq!((audit.row_count, audit.sql, audit.data_source_id), (Some(3), None, None));

    // Unavailable connections are refused, and audited.
    let refused = tool_error(env.query(&caller, &no_password.id, "select 1").await);
    assert!(refused.contains("password isn't saved"), "{refused}");
    assert_eq!(env.test.last_audit().decision, Decision::Denied);
    let refused = tool_error(env.query(&caller, "uri", "select 1").await);
    assert!(refused.contains("file: URI"), "{refused}");
}

#[tokio::test]
async fn refuses_and_audits_connections_the_client_may_not_use() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let hidden = env.source("hidden").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);

    for connection in [hidden.id.as_str(), "hidden", "HIDDEN"] {
        let refused = tool_error(env.query(&caller, connection, "select * from t").await);
        assert!(refused.starts_with(&format!("No connection '{connection}' is available")), "{refused}");
        let audit = env.test.last_audit();
        assert_eq!(audit.decision, Decision::Denied);
        // The audit row says which data source it was; the client learns nothing.
        assert_eq!(audit.data_source_id.as_deref(), Some(hidden.id.as_str()));
        assert_eq!(audit.data_source_name.as_deref(), Some("hidden"));
        assert_eq!(audit.sql.as_deref(), Some("select * from t"));
        assert_eq!(audit.error.as_deref(), Some(refused.as_str()));
    }
    tool_error(env.query(&caller, "nowhere", "select 1").await);
    assert_eq!(env.test.last_audit().data_source_id, None);

    // Revoking the client takes every grant with it, at once.
    env.query(&caller, &shop.id, "select 1").await.unwrap();
    env.test.store.mcp_client_revoke(&caller.client_id).unwrap();
    tool_error(env.query(&caller, &shop.id, "select 1").await);
    assert!(env.server.list_connections(&caller).await.unwrap().connections.is_empty());
}

#[tokio::test]
async fn finds_connections_by_unique_name() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let first = env.source("twin").await;
    let second = env.test.save(DataSource { id: String::new(), ..first.clone() });
    let (caller, _) =
        env.test.client("claude", &[(&shop, Access::Read), (&first, Access::Read), (&second, Access::Read)]);

    assert_eq!(env.query(&caller, "shop", "select 1").await.unwrap().rows, [[json!(1)]]);
    assert_eq!(env.query(&caller, "Shop", "select 1").await.unwrap().rows, [[json!(1)]]);
    assert_eq!(env.test.last_audit().data_source_id.as_deref(), Some(shop.id.as_str()));

    let ambiguous = tool_error(env.query(&caller, "twin", "select 1").await);
    assert!(ambiguous.starts_with("2 connections this client may use are named 'twin'"), "{ambiguous}");
    assert!(ambiguous.contains(&first.id) && ambiguous.contains(&second.id), "{ambiguous}");
    assert_eq!(env.test.last_audit().decision, Decision::Denied);
    // Ids always work.
    env.query(&caller, &second.id, "select 1").await.unwrap();
}

#[tokio::test]
async fn query_runs_reads_only() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    let refused = tool_error(env.query(&caller, "shop", "delete from t").await);
    assert_eq!(refused, "This statement writes (DELETE); use the execute tool, the user must approve it.");
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Denied, Some("DELETE")));

    let refused = tool_error(env.query(&caller, "shop", "select 1; delete from t").await);
    assert!(refused.starts_with("Only one statement per call"), "{refused}");
    assert_eq!(env.test.last_audit().statement_kind.as_deref(), Some("Multiple statements"));
    let refused = tool_error(env.query(&caller, "shop", "attach database 'x.db' as x").await);
    assert!(refused.starts_with("IdeDB refuses this statement (ATTACH"), "{refused}");

    assert_eq!(names(&shop).await.len(), 3);
    assert_eq!(env.approvals_requested(), 0);
}

#[tokio::test]
async fn query_runs_unparsed_statements_that_look_like_reads() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);

    // Too long for the classifier to parse, but SQLite runs it.
    let sql = format!("select count(*) from t where id in ({})", vec!["1"; 6000].join(", "));
    let classification = idedb_sql::classify(Engine::Sqlite, &sql);
    assert_eq!((classification.kind, classification.looks_like_read), (Kind::Write(WriteKind::Unparsed), true));

    let result = env.query(&caller, "shop", &sql).await.unwrap();
    assert_eq!(result.rows, [[json!(1)]]);
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Allowed, Some("unparsed read")));
}

#[tokio::test]
async fn returns_rows_as_json_with_limits() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);

    let args = QueryArgs {
        connection: "shop".into(),
        sql: "select id, name from t order by id".into(),
        schema: Some("main".into()),
        max_rows: Some(2),
    };
    let result = env.server.query(&caller, args, &CancellationToken::new()).await.unwrap();
    assert_eq!(serde_json::to_value(&result.columns).unwrap(), json!([{"name": "id", "type": "integer"}, {"name": "name", "type": "text"}]));
    assert_eq!(result.rows, [[json!(1), json!("a")], [json!(2), json!("b")]]);
    assert_eq!((result.row_count, result.truncated), (2, true));
    let audit = env.test.last_audit();
    assert_eq!((audit.row_count, audit.truncated, audit.statement_kind.as_deref()), (Some(2), true, Some("SELECT")));
    assert!(audit.elapsed_ms.is_some());

    let sql = "select 9007199254740993, -9007199254740991, 1e999, replace(hex(zeroblob(1500)), '00', 'xy'), x'00ff'";
    let values = env.query(&caller, "shop", sql).await.unwrap();
    assert!(!values.truncated);
    let row = &values.rows[0];
    assert_eq!(row[0], json!("9007199254740993"));
    assert_eq!(row[1], json!(-9007199254740991i64));
    assert_eq!(row[2], json!("Infinity"));
    assert_eq!(row[3], json!(format!("{}…[truncated, 3000 chars]", "xy".repeat(1000))));
    assert_eq!(row[4], json!("<binary 2 bytes: 0x00ff>"));

    // The rows past about 256 KB are left out.
    let sql = "with recursive c(x) as (select 1 union all select x + 1 from c limit 1000)
               select x, hex(zeroblob(500)) from c";
    let args = QueryArgs { connection: "shop".into(), sql: sql.into(), schema: None, max_rows: Some(1000) };
    let big = env.server.query(&caller, args, &CancellationToken::new()).await.unwrap();
    assert!(big.truncated);
    assert!((200..300).contains(&big.rows.len()), "{}", big.rows.len());
    let size = serde_json::to_vec(&big.rows).unwrap().len();
    assert!((256 * 1024..260 * 1024).contains(&size), "{size}");
}

#[tokio::test]
async fn cancels_statements_at_the_timeout() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);
    env.settings(|s| s.statement_timeout_secs = 1);

    let endless = "with recursive c(x) as (select 1 union all select x + 1 from c) select count(*) from c";
    let refused = tool_error(env.query(&caller, "shop", endless).await);
    assert!(refused.starts_with("The statement was cancelled after 1s, IdeDB's statement timeout."), "{refused}");
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.error.as_deref()), (Decision::Allowed, Some(refused.as_str())));
    assert!(audit.elapsed_ms.is_some_and(|ms| ms >= 1000), "{:?}", audit.elapsed_ms);

    // The pooled session is fine afterwards.
    assert_eq!(env.query(&caller, "shop", "select count(*) from t").await.unwrap().rows, [[json!(3)]]);
}

#[tokio::test]
async fn cancelling_a_query_stops_it() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);

    let endless = "with recursive c(x) as (select 1 union all select x + 1 from c) select count(*) from c";
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stop.cancel();
    });
    let args = QueryArgs { connection: "shop".into(), sql: endless.into(), schema: None, max_rows: None };
    let refused = tool_error(env.server.query(&caller, args, &cancel).await);
    assert!(refused.starts_with("The call was cancelled"), "{refused}");
    assert_eq!(env.test.last_audit().error.as_deref(), Some(refused.as_str()));
}

#[tokio::test]
async fn explores_tables() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);
    let connection = || shop.id.clone();

    let schemas = env
        .server
        .list_schemas(&caller, ListSchemasArgs { connection: connection(), include_system: false })
        .await
        .unwrap();
    assert_eq!(schemas.default_schema.as_deref(), Some("main"));
    assert_eq!(schemas.schemas.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["main"]);

    let tables = env.server.list_tables(&caller, ListTablesArgs { connection: connection(), schema: None }).await.unwrap();
    assert_eq!(tables.schema, "main");
    let listed: Vec<(&str, TableKind)> = tables.tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(listed, [("t", TableKind::Table), ("u", TableKind::Table)]);
    assert_eq!(env.test.last_audit().row_count, Some(2));

    let args = DescribeTableArgs { connection: connection(), table: "T".into(), schema: None };
    let t = env.server.describe_table(&caller, args).await.unwrap();
    let json = serde_json::to_value(&t).unwrap();
    assert_eq!(json["name"], "t");
    assert_eq!(json["columns"][1], json!({"name": "name", "type": "text", "nullable": false, "generated": false}));
    assert_eq!(json["columns"][0]["primaryKey"], 1);
    assert_eq!(json["referencedBy"], json!([{"name": "fk_u_0", "table": "u", "columns": ["t_id"], "referencedColumns": ["id"]}]));
    let audit = env.test.last_audit();
    assert_eq!((audit.tool.as_str(), audit.decision, audit.sql), ("describe_table", Decision::Allowed, None));

    let u = env
        .server
        .describe_table(&caller, DescribeTableArgs { connection: connection(), table: "u".into(), schema: None })
        .await
        .unwrap();
    assert_eq!((u.foreign_keys.len(), u.referenced_by.len()), (1, 0));

    let missing = DescribeTableArgs { connection: connection(), table: "nope".into(), schema: None };
    let refused = tool_error(env.server.describe_table(&caller, missing).await);
    assert!(refused.contains("There is no table or view 'nope' in schema 'main'"), "{refused}");
    assert_eq!(env.test.last_audit().decision, Decision::Denied);
}

#[tokio::test]
async fn pools_one_session_per_client_and_connection() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let other = env.source("other").await;
    let (claude, _) = env.test.client("claude", &[(&shop, Access::Read), (&other, Access::Read)]);
    let (cursor, _) = env.test.client("cursor", &[(&shop, Access::Read)]);

    env.query(&claude, "shop", "select 1").await.unwrap();
    env.query(&claude, "shop", "select 2").await.unwrap();
    assert_eq!(env.server.open_sessions(), 1);
    env.query(&claude, "other", "select 1").await.unwrap();
    env.query(&cursor, "shop", "select 1").await.unwrap();
    assert_eq!(env.server.open_sessions(), 3);

    env.server.close_data_source(&shop.id).await;
    assert_eq!(env.server.open_sessions(), 1);
    env.server.close_client(&claude.client_id).await;
    assert_eq!(env.server.open_sessions(), 0);
    env.query(&claude, "shop", "select 1").await.unwrap();
    assert_eq!(env.server.open_sessions(), 1);
}

#[tokio::test]
async fn an_approved_write_runs_and_is_audited() {
    let env = Env::new();
    let mut shop = env.source("shop").await;
    shop.color = Some("#e5484d".into());
    let shop = env.test.save(shop);
    let (mut caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);
    caller.client_info = Some(ClientInfo { name: "claude-code".into(), version: Some("2.1.0".into()) });
    caller.protocol_version = Some("2026-07-28".into());
    caller.transport = Transport::Bridge;
    caller.session_key = Some("bridge-1".into());

    let answered = env.answer_next(true);
    let args = ExecuteArgs {
        connection: "shop".into(),
        sql: "delete from t where name = 'b'".into(),
        schema: None,
        reason: Some("remove the duplicate".into()),
    };
    let result = env.server.execute(&caller, args, |_| {}, &CancellationToken::new()).await.unwrap();
    assert_eq!((result.row_count, result.columns, result.rows), (1, None, None));
    assert_eq!(names(&shop).await, [r#"Text("a")"#, r#"Text("c")"#]);

    let request = answered.await.unwrap();
    assert_eq!((request.client_name.as_str(), &request.client_info), ("claude", &caller.client_info));
    assert_eq!((request.data_source_id.as_str(), request.data_source_name.as_str()), (shop.id.as_str(), "shop"));
    assert_eq!(request.data_source_color.as_deref(), Some("#e5484d"));
    assert_eq!((request.summary.as_str(), request.write_kind), ("DELETE", WriteKind::Dml));
    assert_eq!(request.reason.as_deref(), Some("remove the duplicate"));
    assert!(request.expires_at > request.requested_at);
    let json = serde_json::to_value(McpEvent::ApprovalRequested(request)).unwrap();
    assert_eq!((json["kind"].as_str(), json["writeKind"].as_str()), (Some("approvalRequested"), Some("dml")));
    assert!(env.server.pending_approvals().is_empty());

    let audit = env.test.last_audit();
    assert_eq!((audit.tool.as_str(), audit.decision), ("execute", Decision::Approved));
    assert_eq!((audit.row_count, audit.statement_kind.as_deref()), (Some(1), Some("DELETE")));
    assert_eq!(audit.reason.as_deref(), Some("remove the duplicate"));
    assert!(audit.approval_wait_ms.is_some() && audit.elapsed_ms.is_some());
    assert_eq!((audit.client_id.as_deref(), audit.client_name.as_str()), (Some(caller.client_id.as_str()), "claude"));
    assert_eq!((audit.client_info_name.as_deref(), audit.client_info_version.as_deref()), (Some("claude-code"), Some("2.1.0")));
    assert_eq!((audit.protocol_version.as_deref(), audit.transport), (Some("2026-07-28"), Transport::Bridge));
    assert_eq!(audit.session_key.as_deref(), Some("bridge-1"));
    assert_eq!(audit.data_source_name.as_deref(), Some("shop"));

    // Requested, resolved, then audited.
    let kinds: Vec<&str> = env
        .test
        .events()
        .iter()
        .filter_map(|e| match e {
            McpEvent::ApprovalRequested(_) => Some("requested"),
            McpEvent::ApprovalResolved { decision: Decision::Approved, .. } => Some("approved"),
            McpEvent::Audit(_) => Some("audit"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["requested", "approved", "audit"]);
}

#[tokio::test]
async fn returns_the_rows_of_returning() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    env.answer_next(true);
    let result = env.execute(&caller, "shop", "insert into t (name) values ('d'), ('e') returning id, name").await.unwrap();
    assert_eq!(result.rows, Some(vec![vec![json!(4), json!("d")], vec![json!(5), json!("e")]]));
    assert_eq!((result.row_count, result.truncated), (2, false));
    assert_eq!(result.columns.unwrap().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "name"]);
    assert_eq!(names(&shop).await.len(), 5);
}

#[tokio::test]
async fn a_rejected_write_does_not_run() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    env.answer_next(false);
    let refused = tool_error(env.execute(&caller, "shop", "delete from t").await);
    assert!(refused.starts_with("The user rejected this statement"), "{refused}");
    assert_eq!(names(&shop).await.len(), 3);
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.elapsed_ms), (Decision::Rejected, None));
    assert!(audit.approval_wait_ms.is_some());
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_write_times_out() {
    let env = Env::new();
    // No session is opened before the approval, so the paused clock only
    // waits on timers.
    let shop = env.test.save_sqlite("shop", &env.dir.path().join("shop.db"));
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    let reports = Arc::new(Mutex::new(Vec::new()));
    let seen = reports.clone();
    let args = ExecuteArgs { connection: "shop".into(), sql: "delete from t".into(), schema: None, reason: None };
    let server = env.server.clone();
    let waiting = tokio::spawn(async move {
        server.execute(&caller, args, move |p: Progress| seen.lock().unwrap().push(p), &CancellationToken::new()).await
    });
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(env.server.pending_approvals().len(), 1);

    let refused = tool_error(waiting.await.unwrap());
    assert!(refused.starts_with("Nobody approved the statement in IdeDB within 120s"), "{refused}");
    assert!(env.server.pending_approvals().is_empty());
    let audit = env.test.last_audit();
    assert_eq!(audit.decision, Decision::Timeout);
    assert!(audit.approval_wait_ms.is_some_and(|ms| (120_000..121_000).contains(&ms)), "{:?}", audit.approval_wait_ms);

    // Every 15 s while waiting.
    let reports = reports.lock().unwrap();
    let waited: Vec<u64> = reports.iter().map(|p| p.waited_secs).collect();
    assert_eq!(waited, [15, 30, 45, 60, 75, 90, 105]);
    assert_eq!(reports[0].timeout_secs, 120);
    assert!(reports[0].message.contains("15 of 120 s"), "{}", reports[0].message);
}

#[tokio::test]
async fn cancelling_withdraws_a_pending_write() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    let mut events = env.test.subscribe();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        while !matches!(events.recv().await.unwrap(), McpEvent::ApprovalRequested(_)) {}
        stop.cancel();
    });
    let args = ExecuteArgs { connection: "shop".into(), sql: "delete from t".into(), schema: None, reason: None };
    let refused = tool_error(env.server.execute(&caller, args, |_| {}, &cancel).await);
    assert!(refused.starts_with("The call was cancelled before the user answered"), "{refused}");
    assert_eq!(env.test.last_audit().decision, Decision::Withdrawn);
    assert!(env.server.pending_approvals().is_empty());
    assert_eq!(names(&shop).await.len(), 3);
}

#[tokio::test]
async fn dropping_the_call_withdraws_a_pending_write() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    let mut events = env.test.subscribe();
    let server = env.server.clone();
    let call = tokio::spawn(async move { env_execute(&server, &caller, "delete from t").await });
    while !matches!(events.recv().await.unwrap(), McpEvent::ApprovalRequested(_)) {}
    call.abort();

    // The call's own task still resolves and audits it.
    loop {
        if let McpEvent::Audit(audit) = events.recv().await.unwrap() {
            assert_eq!(audit.decision, Decision::Withdrawn);
            break;
        }
    }
    assert!(env.server.pending_approvals().is_empty());
}

async fn env_execute(server: &McpServer, caller: &Caller, sql: &str) -> Result<ExecuteOutput, CallError> {
    let args = ExecuteArgs { connection: "shop".into(), sql: sql.into(), schema: None, reason: None };
    server.execute(caller, args, |_| {}, &CancellationToken::new()).await
}

#[tokio::test]
async fn writes_without_write_access_are_refused_without_asking() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let guarded = env.source("guarded").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read), (&guarded, Access::Write)]);
    env.test.store.mcp_set_never_write(&guarded.id, true).unwrap();

    let refused = tool_error(env.execute(&caller, "shop", "delete from t").await);
    assert!(refused.starts_with("This client may only read 'shop'"), "{refused}");
    assert_eq!(env.test.last_audit().decision, Decision::Denied);

    let refused = tool_error(env.execute(&caller, "guarded", "delete from t").await);
    assert!(refused.starts_with("'guarded' is marked never-write in IdeDB"), "{refused}");
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.approval_wait_ms), (Decision::Denied, None));

    let refused = tool_error(env.execute(&caller, "guarded", "begin").await);
    assert!(refused.contains("transaction control"), "{refused}");

    assert_eq!(env.approvals_requested(), 0);
    assert_eq!(names(&shop).await.len() + names(&guarded).await.len(), 6);
}

#[tokio::test]
async fn access_taken_away_while_waiting_stops_the_write() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);

    let mut events = env.test.subscribe();
    let (test, server, client_id, source) = (env.test.clone(), env.server.clone(), caller.client_id.clone(), shop.clone());
    tokio::spawn(async move {
        loop {
            if let McpEvent::ApprovalRequested(request) = events.recv().await.unwrap() {
                test.grant(&client_id, &[(&source, Access::Read)]);
                server.answer_approval(request.id, true);
                return;
            }
        }
    });
    let refused = tool_error(env.execute(&caller, "shop", "delete from t").await);
    assert!(refused.starts_with("While the statement waited for approval, the user changed"), "{refused}");
    assert_eq!(names(&shop).await.len(), 3);
    let audit = env.test.last_audit();
    assert_eq!(audit.decision, Decision::Denied);
    assert!(audit.approval_wait_ms.is_some());
}

#[tokio::test]
async fn a_read_sent_to_execute_needs_no_approval() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Read)]);

    let result = env.execute(&caller, "shop", "select count(*) as n from t").await.unwrap();
    assert_eq!((result.row_count, result.rows), (1, Some(vec![vec![json!(3)]])));
    assert_eq!(env.approvals_requested(), 0);
    let audit = env.test.last_audit();
    assert_eq!((audit.tool.as_str(), audit.decision), ("execute", Decision::Allowed));
}

#[tokio::test(start_paused = true)]
async fn authenticates_tokens_and_marks_clients_seen() {
    let env = Env::new();
    let (caller, token) = env.test.client("claude", &[]);
    let seen = || env.test.events().iter().filter(|e| matches!(e, McpEvent::ClientSeen { .. })).count();

    let authenticated = env.server.authenticate(&token).unwrap().unwrap();
    assert_eq!(authenticated, caller);
    assert_eq!(seen(), 1);
    let stored = env.test.store.mcp_client(&caller.client_id).unwrap().unwrap();
    match &env.test.events()[0] {
        McpEvent::ClientSeen { client_id, at } => {
            assert_eq!((client_id, Some(at)), (&caller.client_id, stored.last_seen_at.as_ref()));
        }
        other => panic!("{other:?}"),
    }
    assert!(env.server.authenticate("idedb_wrong").unwrap().is_none());

    // At most every 30 s…
    tokio::time::advance(Duration::from_secs(10)).await;
    env.server.authenticate(&token).unwrap().unwrap();
    assert_eq!(seen(), 1);
    tokio::time::advance(Duration::from_secs(21)).await;
    env.server.authenticate(&token).unwrap().unwrap();
    assert_eq!(seen(), 2);
    // …or when the client declares something new.
    let declared = Caller { client_info: Some(ClientInfo { name: "cursor".into(), version: None }), ..caller.clone() };
    env.server.list_connections(&declared).await.unwrap();
    assert_eq!(seen(), 3);
    assert_eq!(env.test.store.mcp_client(&caller.client_id).unwrap().unwrap().last_client_name.as_deref(), Some("cursor"));
    env.server.list_connections(&declared).await.unwrap();
    assert_eq!(seen(), 3);

    env.test.store.mcp_client_revoke(&caller.client_id).unwrap();
    assert!(env.server.authenticate(&token).unwrap().is_none());
}

#[tokio::test]
async fn progress_is_optional_and_counted() {
    // A write answered at once never reports progress.
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    env.answer_next(true);
    let args = ExecuteArgs { connection: "shop".into(), sql: "update t set name = 'z'".into(), schema: None, reason: None };
    let progress = move |_: Progress| {
        counted.fetch_add(1, Ordering::Relaxed);
    };
    let result = env.server.execute(&caller, args, progress, &CancellationToken::new()).await.unwrap();
    assert_eq!(result.row_count, 3);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

/// Leaves a journal next to the data source's file, as a writer that
/// crashed mid-transaction does: SQLite must roll it back before reading
/// anything, which a read-only session cannot, so it refuses even a SELECT
/// as a write. A read-write session rolls it back (this one holds nothing).
fn leave_a_hot_journal(env: &Env, source: &DataSource) {
    std::fs::write(format!("{}-journal", source.params.path), b"x").unwrap();
    assert!(env.dir.path().join("shop.db-journal").exists());
}

#[tokio::test]
async fn a_read_the_engine_refuses_as_a_write_needs_approval() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);
    // The pooled read-only session is open before the journal appears.
    env.query(&caller, "shop", "select 1").await.unwrap();
    leave_a_hot_journal(&env, &shop);
    let sql = "select count(*) from t";

    // query refuses it, and says where to go.
    let refused = tool_error(env.query(&caller, "shop", sql).await);
    assert!(refused.starts_with("The database refused this statement because it writes"), "{refused}");
    assert!(refused.contains("attempt to write a readonly database"), "{refused}");
    assert!(refused.ends_with("Use the execute tool instead; the user must approve it."), "{refused}");
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Denied, Some("SELECT (engine refused as write)")));
    assert!(audit.elapsed_ms.is_some());
    assert_eq!(env.approvals_requested(), 0);

    // execute takes it through approval, onto a read-write session.
    let answered = env.answer_next(true);
    let result = env.execute(&caller, "shop", sql).await.unwrap();
    assert_eq!(result.rows, Some(vec![vec![json!(3)]]));
    let request = answered.await.unwrap();
    assert_eq!((request.summary.as_str(), request.write_kind), ("SELECT (engine refused as write)", WriteKind::Other));
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Approved, Some("SELECT (engine refused as write)")));
    assert!(audit.approval_wait_ms.is_some());
    assert_eq!(env.test.audit().len(), 3);
    // The read-write session rolled the journal back, so reads work again.
    assert!(!env.dir.path().join("shop.db-journal").exists());
    assert_eq!(env.query(&caller, "shop", sql).await.unwrap().rows, [[json!(3)]]);
}

#[tokio::test]
async fn a_read_the_engine_refuses_follows_the_rules_for_writes() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (reader, _) = env.test.client("reader", &[(&shop, Access::Read)]);
    let (writer, _) = env.test.client("writer", &[(&shop, Access::Write)]);
    env.query(&reader, "shop", "select 1").await.unwrap();
    env.query(&writer, "shop", "select 1").await.unwrap();
    leave_a_hot_journal(&env, &shop);

    let refused = tool_error(env.execute(&reader, "shop", "select * from t").await);
    assert!(refused.starts_with("The database refused this statement as a write"), "{refused}");
    assert!(refused.contains("This client may only read 'shop'"), "{refused}");
    let audit = env.test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Denied, Some("SELECT (engine refused as write)")));

    env.test.store.mcp_set_never_write(&shop.id, true).unwrap();
    let refused = tool_error(env.execute(&writer, "shop", "select * from t").await);
    assert!(refused.contains("'shop' is marked never-write in IdeDB"), "{refused}");
    assert_eq!(env.approvals_requested(), 0);
}

#[tokio::test]
async fn approved_writes_have_a_timeout_of_their_own() {
    let env = Env::new();
    let shop = env.source("shop").await;
    let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);
    env.settings(|s| {
        s.statement_timeout_secs = 1;
        s.write_timeout_secs = 2;
    });

    env.answer_next(true);
    let endless = "insert into t (name) with recursive c(x) as (select 1 union all select x + 1 from c) select 'x' from c";
    let refused = tool_error(env.execute(&caller, "shop", endless).await);
    assert!(refused.starts_with("The statement was cancelled after 2s, IdeDB's write timeout."), "{refused}");
    let audit = env.test.last_audit();
    assert_eq!(audit.decision, Decision::Approved);
    assert!(audit.elapsed_ms.is_some_and(|ms| ms >= 2000), "{:?}", audit.elapsed_ms);
    // Stopped, and undone.
    assert_eq!(names(&shop).await.len(), 3);
}

#[tokio::test]
async fn revoking_or_deleting_a_client_withdraws_its_pending_writes() {
    for delete in [false, true] {
        let env = Env::new();
        let shop = env.source("shop").await;
        let (caller, _) = env.test.client("claude", &[(&shop, Access::Write)]);
        let (other, _) = env.test.client("cursor", &[(&shop, Access::Write)]);

        let mut events = env.test.subscribe();
        let server = env.server.clone();
        let (waiting, other_waiting) = {
            let (server, caller, other) = (server.clone(), caller.clone(), other.clone());
            (
                tokio::spawn({
                    let server = server.clone();
                    async move { env_execute(&server, &caller, "delete from t").await }
                }),
                tokio::spawn(async move { env_execute(&server, &other, "delete from t").await }),
            )
        };
        let mut requested = 0;
        while requested < 2 {
            if matches!(events.recv().await.unwrap(), McpEvent::ApprovalRequested(_)) {
                requested += 1;
            }
        }

        if delete {
            env.test.store.mcp_client_delete(&caller.client_id).unwrap();
        } else {
            env.test.store.mcp_client_revoke(&caller.client_id).unwrap();
        }
        server.close_client(&caller.client_id).await;

        let refused = tool_error(waiting.await.unwrap());
        assert!(refused.starts_with("This client's access was revoked in IdeDB"), "{refused}");
        let audit = env.test.last_audit();
        assert_eq!((audit.decision, audit.client_id.as_deref()), (Decision::Withdrawn, Some(caller.client_id.as_str())));
        assert!(audit.approval_wait_ms.is_some());
        let resolved = env.test.events().into_iter().any(|e| {
            matches!(e, McpEvent::ApprovalResolved { decision: Decision::Withdrawn, .. })
        });
        assert!(resolved);

        // The other client's approval is still pending, and still answerable.
        let pending = server.pending_approvals();
        assert_eq!(pending.iter().map(|r| r.client_id.as_str()).collect::<Vec<_>>(), [other.client_id.as_str()]);
        assert!(server.answer_approval(pending[0].id, false));
        tool_error(other_waiting.await.unwrap());
        assert_eq!(names(&shop).await.len(), 3);
    }
}
