//! The tools on real servers: what only a server engine can show. They read
//! `IDEDB_PG_URL` and skip themselves without it.
//!
//! MySQL has no case here: with binary logging on (the default), creating a
//! function that writes needs SUPER, which the test user lacks. The read
//! path's own tests show MySQL's refusal is recognized.

use idedb_core::{Engine, Value};
use idedb_mcp::testing::{TestHost, exec};
use idedb_mcp::{CallError, CancellationToken, Decision, ExecuteArgs, McpEvent, McpServer, QueryArgs};
use idedb_store::Access;
use serde_json::json;

fn tool_error<T: std::fmt::Debug>(result: Result<T, CallError>) -> String {
    match result {
        Err(CallError::Tool(e)) => e.message,
        other => panic!("expected a tool error, got {other:?}"),
    }
}

/// A SELECT that writes through a function: the classifier cannot tell,
/// the read-only transaction refuses it, and `execute` asks the user.
#[tokio::test]
async fn postgres_takes_a_select_that_writes_through_approval() {
    let test = TestHost::new();
    let Some((source, password)) = test.save_server("IDEDB_PG_URL", Engine::Postgres, None) else { return };
    let table = format!("idedb_mcp_probe_escalate_{}", std::process::id());
    let function = format!("{table}_add");
    exec(&source, Some(&password), &format!("create table {table} (id int)")).await;
    exec(
        &source,
        Some(&password),
        &format!("create function {function}() returns int language sql as $$ insert into {table} values (1); select 1 $$"),
    )
    .await;
    let server = McpServer::new(test.clone());
    let (caller, _) = test.client("claude", &[(&source, Access::Write)]);
    let sql = format!("select {function}()");
    assert_eq!(idedb_sql::classify(Engine::Postgres, &sql).kind, idedb_sql::Kind::Read);

    let args = QueryArgs { connection: source.id.clone(), sql: sql.clone(), schema: None, max_rows: None };
    let refused = tool_error(server.query(&caller, args, &CancellationToken::new()).await);
    assert!(refused.contains("in a read-only transaction"), "{refused}");
    assert!(refused.ends_with("Use the execute tool instead; the user must approve it."), "{refused}");
    assert_eq!(test.last_audit().statement_kind.as_deref(), Some("SELECT (engine refused as write)"));

    let mut events = test.subscribe();
    let approver = server.clone();
    tokio::spawn(async move {
        loop {
            if let McpEvent::ApprovalRequested(request) = events.recv().await.unwrap() {
                approver.answer_approval(request.id, true);
                return;
            }
        }
    });
    let args = ExecuteArgs { connection: source.id.clone(), sql, schema: None, reason: None };
    let result = server.execute(&caller, args, |_| {}, &CancellationToken::new()).await.unwrap();
    assert_eq!(result.rows, Some(vec![vec![json!(1)]]));
    let audit = test.last_audit();
    assert_eq!((audit.decision, audit.statement_kind.as_deref()), (Decision::Approved, Some("SELECT (engine refused as write)")));
    let written = exec(&source, Some(&password), &format!("select count(*) from {table}")).await;
    assert_eq!(written, [[Value::Int(1)]]);

    exec(&source, Some(&password), &format!("drop function {function}()")).await;
    exec(&source, Some(&password), &format!("drop table {table}")).await;
}
