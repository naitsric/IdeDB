//! Integration tests against a real Postgres.
//!
//! Start one with `docker compose up -d --wait` and run with
//! `IDEDB_PG_URL=postgres://idedb:idedb@localhost:54329/idedb cargo test -p idedb-driver-pg`.
//! Without `IDEDB_PG_URL` these tests are skipped.

use idedb_core::testing::{self, collect, rows};
use idedb_core::{
    ApplyOutcome, ColumnInfo, ColumnValue, ConnectOptions, ConnectionParams, Engine, ForeignKey, ObjectKind, QueryEvent,
    RowChange, Session, SslMode, TableRef, Value,
};
use idedb_driver_pg::PgSession;

async fn session() -> Option<PgSession> {
    session_with(ConnectOptions::default()).await
}

async fn session_with(options: ConnectOptions) -> Option<PgSession> {
    let Ok(url) = std::env::var("IDEDB_PG_URL") else {
        eprintln!("IDEDB_PG_URL not set, skipping");
        return None;
    };
    let config: tokio_postgres::Config = url.parse().expect("IDEDB_PG_URL");
    let host = match &config.get_hosts()[0] {
        tokio_postgres::config::Host::Tcp(host) => host.clone(),
        other => panic!("unsupported host {other:?}"),
    };
    let params = ConnectionParams {
        engine: Engine::Postgres,
        host,
        port: config.get_ports().first().copied(),
        user: config.get_user().unwrap_or_default().to_owned(),
        database: config.get_dbname().unwrap_or_default().to_owned(),
        ssl_mode: SslMode::Prefer,
        path: String::new(),
    };
    let password = config.get_password().map(|p| String::from_utf8_lossy(p).into_owned());
    Some(PgSession::connect_with(&params, password.as_deref(), options).await.expect("connect"))
}

#[tokio::test]
async fn reports_server_info() {
    let Some(s) = session().await else { return };
    let info = s.server_info();
    assert_eq!(info.engine, Engine::Postgres);
    assert!(info.version.starts_with("17"), "{}", info.version);
    assert_eq!(info.default_schema.as_deref(), Some("public"));
}

#[tokio::test]
async fn streams_a_million_rows_in_pages() {
    let Some(mut s) = session().await else { return };
    testing::streams_in_pages(
        &mut s,
        "select g, md5(g::text) from generate_series(1, 1000000) g",
        1_000_000,
        2000,
    )
    .await;
}

#[tokio::test]
async fn cancels_a_running_statement() {
    let Some(s) = session().await else { return };
    testing::cancels_a_running_statement(s, "select pg_sleep(30)").await;
}

#[tokio::test]
async fn cancels_between_pages() {
    let Some(mut s) = session().await else { return };
    testing::cancels_between_pages(&mut s, "select g from generate_series(1, 1000000) g", 1000).await;
}

#[tokio::test]
async fn reports_affected_rows_and_errors() {
    let Some(mut s) = session().await else { return };
    testing::reports_affected_rows_and_errors(
        &mut s,
        "create temp table t (id int)",
        "insert into t values (1), (2), (3)",
        "select * from missing_table",
        "\"missing_table\" does not exist",
    )
    .await;
}

#[tokio::test]
async fn decodes_common_types() {
    let Some(mut s) = session().await else { return };
    let events = collect(
        &mut s,
        "select true, 42::int2, 7::int8, 1.5::float8, 12345.678::numeric(10,3), 'héllo'::text,
                '\\x00ff'::bytea, '{\"a\": 1}'::jsonb, '00000000-0000-0000-0000-000000000001'::uuid,
                '2026-09-25'::date, '2026-09-25 10:11:12.5'::timestamp, 'infinity'::timestamp,
                '1 year 2 mons 3 days 04:05:06'::interval, array[1, null, 3], array['a b', 'c'],
                null::text, 'NaN'::numeric",
        10,
    )
    .await;

    let QueryEvent::Columns { columns } = &events[0] else { panic!("{events:?}") };
    assert_eq!(columns[4].type_name, "numeric");
    let row = &rows(&events)[0];
    let text = |s: &str| Value::Text(s.into());
    assert_eq!(
        row,
        &vec![
            Value::Bool(true),
            Value::Int(42),
            Value::Int(7),
            Value::Float(1.5),
            text("12345.678"),
            text("héllo"),
            Value::Bytes(vec![0, 255]),
            text(r#"{"a": 1}"#),
            text("00000000-0000-0000-0000-000000000001"),
            text("2026-09-25"),
            text("2026-09-25 10:11:12.500"),
            text("infinity"),
            text("1 year 2 mons 3 days 04:05:06"),
            text("{1,NULL,3}"),
            text(r#"{"a b",c}"#),
            Value::Null,
            text("NaN"),
        ]
    );
}

/// JSON comes back exactly as the server stores it: no rounding of big or
/// precise numbers, and for `json` the original key order and duplicates.
#[tokio::test]
async fn keeps_json_exactly() {
    let Some(mut s) = session().await else { return };
    let doc = r#"{"id": 12345678901234567890123, "amt": 0.10000000000000000001, "id": 2}"#;
    let events = collect(&mut s, &format!("select '{doc}'::json, '{doc}'::jsonb"), 10).await;
    assert_eq!(
        rows(&events)[0],
        vec![
            Value::Text(doc.into()),
            Value::Text(r#"{"id": 2, "amt": 0.10000000000000000001}"#.into()),
        ]
    );
}

/// Types without a native value decode to the text Postgres prints, and
/// that text reads back into the type (the data editor casts it).
#[tokio::test]
async fn decodes_network_money_time_bit_and_system_types_as_text() {
    let Some(mut s) = session().await else { return };
    let values = [
        ("'127.0.0.1'::inet", "127.0.0.1"),
        ("'192.168.0.10/24'::inet", "192.168.0.10/24"),
        ("'2001:db8::1'::inet", "2001:db8::1"),
        ("'10.0.0.0/8'::cidr", "10.0.0.0/8"),
        ("'08:00:2b:01:02:03'::macaddr", "08:00:2b:01:02:03"),
        ("'08:00:2b:01:02:03:04:05'::macaddr8", "08:00:2b:01:02:03:04:05"),
        ("'-1234.5'::numeric::money", "-1234.50"),
        ("'10:11:12.5+02'::timetz", "10:11:12.5+02"),
        ("'04:05:06-03:30'::timetz", "04:05:06-03:30"),
        ("B'10110'::bit(5)", "10110"),
        ("B'101'::varbit", "101"),
        ("'42'::xid", "42"),
        ("'16/B374D848'::pg_lsn", "16/B374D848"),
        ("array['10.0.0.1'::inet, null]", "{10.0.0.1,NULL}"),
    ];
    for (expr, expected) in values {
        let events = collect(&mut s, &format!("select {expr}"), 10).await;
        assert_eq!(rows(&events), vec![vec![Value::Text(expected.into())]], "{expr}");
    }

    // The displayed text round-trips through the data editor's text cast.
    run_all(
        &mut s,
        &[
            "drop table if exists idedb_text_types",
            "create table idedb_text_types (id int primary key, ip inet, mac macaddr, cash money, at timetz, flags bit(5))",
            "insert into idedb_text_types values (1, null, null, null, null, null)",
        ],
    )
    .await;
    let table = TableRef { schema: "public".into(), name: "idedb_text_types".into() };
    let cv = |column: &str, value: &str| ColumnValue { column: column.into(), value: Value::Text(value.into()) };
    let values = vec![
        cv("ip", "192.168.0.10/24"),
        cv("mac", "08:00:2b:01:02:03"),
        cv("cash", "-1234.50"),
        cv("at", "04:05:06-03:30"),
        cv("flags", "10110"),
    ];
    let key = vec![ColumnValue { column: "id".into(), value: Value::Int(1) }];
    let outcome = s.apply(&table, &[RowChange::Update { key, values: values.clone() }]).await.unwrap();
    let ApplyOutcome::Applied { rows: stored, .. } = outcome else { panic!("{outcome:?}") };
    let stored = stored[0].clone().unwrap();
    assert_eq!(stored[1..], values.iter().map(|v| v.value.clone()).collect::<Vec<_>>()[..]);
}

#[tokio::test]
async fn introspects_a_schema() {
    let Some(mut s) = session().await else { return };
    let setup = "drop schema if exists idedb_introspect cascade;
        create schema idedb_introspect;
        create table idedb_introspect.orders (
            tenant int, id bigint, note text default 'n/a', primary key (tenant, id));
        comment on table idedb_introspect.orders is 'All orders';
        create table idedb_introspect.lines (
            id serial primary key, tenant int not null, order_id bigint not null,
            foreign key (tenant, order_id) references idedb_introspect.orders (tenant, id));
        create view idedb_introspect.recent as select * from idedb_introspect.orders;";
    for statement in setup.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let events = collect(&mut s, statement, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{statement}: {events:?}");
    }

    let schemas = s.schemas().await.unwrap();
    assert!(schemas.iter().any(|x| x.name == "idedb_introspect" && !x.is_system));
    assert!(schemas.iter().any(|x| x.name == "pg_catalog" && x.is_system));

    let model = s.introspect("idedb_introspect").await.unwrap();
    let names: Vec<_> = model.tables.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    assert_eq!(
        names,
        [("lines", ObjectKind::Table), ("orders", ObjectKind::Table), ("recent", ObjectKind::View)]
    );

    let orders = &model.tables[1];
    assert_eq!(orders.comment.as_deref(), Some("All orders"));
    assert_eq!(
        orders.columns,
        vec![
            ColumnInfo { name: "tenant".into(), type_name: "integer".into(), nullable: false, default: None, primary_key: Some(1), comment: None, generated: false },
            ColumnInfo { name: "id".into(), type_name: "bigint".into(), nullable: false, default: None, primary_key: Some(2), comment: None, generated: false },
            ColumnInfo { name: "note".into(), type_name: "text".into(), nullable: true, default: Some("'n/a'::text".into()), primary_key: None, comment: None, generated: false },
        ]
    );

    let lines = &model.tables[0];
    assert_eq!(lines.columns[0].default.as_deref(), Some("nextval('idedb_introspect.lines_id_seq'::regclass)"));
    assert!(lines.columns[0].generated, "a serial key is generated");
    assert_eq!(
        lines.foreign_keys,
        vec![ForeignKey {
            name: "lines_tenant_order_id_fkey".into(),
            columns: vec!["tenant".into(), "order_id".into()],
            referenced_schema: "idedb_introspect".into(),
            referenced_table: "orders".into(),
            referenced_columns: vec!["tenant".into(), "id".into()],
        }]
    );
}

#[tokio::test]
async fn locates_syntax_errors_in_chars() {
    let Some(mut s) = session().await else { return };
    // Multi-byte text before the error proves the offset is in chars, not bytes.
    let events = collect(&mut s, "select 'ñandú' from from t", 10).await;
    let Some(QueryEvent::Error { message, position, .. }) = events.last() else { panic!("{events:?}") };
    assert!(message.contains("syntax error"), "{message}");
    assert_eq!(*position, Some(20), "{message}");
}

#[tokio::test]
async fn applies_row_changes() {
    let Some(mut s) = session().await else { return };
    testing::applies_row_changes(&mut s, "public").await;
}

/// Columns the database fills itself are flagged, so a duplicated row leaves them out.
#[tokio::test]
async fn flags_generated_columns() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "drop table if exists idedb_generated",
            "create table idedb_generated (
                id int generated by default as identity primary key,
                seq bigserial,
                total int generated always as (id * 2) stored,
                plain int default 7,
                at timestamptz default now())",
        ],
    )
    .await;
    let model = s.introspect("public").await.unwrap();
    let table = model.tables.iter().find(|t| t.name == "idedb_generated").unwrap();
    let flags: Vec<_> = table.columns.iter().map(|c| (c.name.as_str(), c.generated)).collect();
    assert_eq!(flags, [("id", true), ("seq", true), ("total", true), ("plain", false), ("at", false)]);
}

#[tokio::test]
async fn fetches_on_demand() {
    let Some(mut s) = session().await else { return };
    testing::fetches_on_demand(&mut s, "select g from generate_series(1, 23) g", 23).await;
}

#[tokio::test]
async fn closes_open_results() {
    let Some(mut s) = session().await else { return };
    testing::closes_open_results(&mut s, "select g from generate_series(1, 10) g").await;
}

#[tokio::test]
async fn cancels_fetch_more() {
    let Some(mut s) = session().await else { return };
    testing::cancels_fetch_more(&mut s, "select g from generate_series(1, 5000000) g").await;
}

#[tokio::test]
async fn open_results_respect_user_transactions() {
    let Some(mut s) = session().await else { return };
    testing::open_results_respect_user_transactions(&mut s, "public").await;
}

/// A statement a cursor cannot hold is read whole even with a fetch limit,
/// and its effects are committed as in autocommit.
#[tokio::test]
async fn reads_statements_a_cursor_cannot_hold_whole() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["drop table if exists idedb_returning", "create table idedb_returning (id int)"]).await;
    let events = testing::collect_first(&mut s, "insert into idedb_returning select g from generate_series(1, 20) g returning id", 5, 100).await;
    assert_eq!(rows(&events).len(), 20, "{events:?}");
    assert_eq!(testing::done(&events), (20, false, false));
    let count = collect(&mut s, "select count(*) from idedb_returning", 10).await;
    assert_eq!(rows(&count), vec![vec![Value::Int(20)]], "{count:?}");
}

#[tokio::test]
async fn respects_user_transactions() {
    let Some(mut s) = session().await else { return };
    testing::respects_user_transactions(&mut s, "public").await;
}

/// Ends `victim`'s connection from another session, as a server restart or
/// an idle timeout would.
async fn terminate(victim: &mut PgSession) {
    let pid = rows(&collect(victim, "select pg_backend_pid()", 10).await)[0][0].clone();
    let Value::Int(pid) = pid else { panic!("{pid:?}") };
    let mut killer = session().await.unwrap();
    run_all(&mut killer, &[&format!("select pg_terminate_backend({pid}, 5000)")]).await;
}

fn last_error(events: &[QueryEvent]) -> (&str, bool) {
    match events.last() {
        Some(QueryEvent::Error { message, in_transaction, .. }) => (message, *in_transaction),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// A lost connection is replaced with the console's search path restored,
/// and the user is told what was lost; the statement that finds out is not
/// silently run on the fresh session.
#[tokio::test]
async fn reconnects_with_the_search_path() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["create schema if not exists idedb_reconnect"]).await;
    s.set_schema("idedb_reconnect").await.unwrap();
    terminate(&mut s).await;

    let events = collect(&mut s, "select 1", 10).await;
    let (message, in_transaction) = last_error(&events);
    assert!(message.contains("re-established") && message.contains("`idedb_reconnect`"), "{message}");
    assert!(!in_transaction);
    assert_eq!(
        rows(&collect(&mut s, "select current_schema()", 10).await),
        vec![vec![Value::Text("idedb_reconnect".into())]]
    );
}

#[tokio::test]
async fn reports_a_transaction_lost_with_the_connection() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "drop table if exists idedb_lost_tx",
            "create table idedb_lost_tx (id int primary key, v int not null)",
            "insert into idedb_lost_tx values (1, 0)",
            "begin",
            "update idedb_lost_tx set v = 1",
        ],
    )
    .await;
    terminate(&mut s).await;

    let events = collect(&mut s, "commit", 10).await;
    let (message, in_transaction) = last_error(&events);
    assert!(message.contains("rolled back"), "{message}");
    assert!(!in_transaction);
    assert_eq!(rows(&collect(&mut s, "select v from idedb_lost_tx", 10).await), vec![vec![Value::Int(0)]]);
}

/// Statements a cursor cannot run still page inside the user's transaction.
#[tokio::test]
async fn streams_non_cursor_statements_inside_a_user_transaction() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["drop table if exists idedb_returning", "create table idedb_returning (id int)"]).await;
    run_all(&mut s, &["begin"]).await;
    let events = collect(&mut s, "insert into idedb_returning select g from generate_series(1, 5) g returning id", 2).await;
    assert_eq!(rows(&events).len(), 5, "{events:?}");
    assert!(matches!(events.last(), Some(QueryEvent::Done { in_transaction: true, .. })), "{events:?}");
    run_all(&mut s, &["rollback"]).await;
    let count = rows(&collect(&mut s, "select count(*) from idedb_returning", 10).await);
    assert_eq!(count, vec![vec![Value::Int(0)]]);
}

/// The driver asks the server whether a transaction block is open, so it
/// also knows about blocks it never saw start (a failed COMMIT, `COMMIT AND
/// CHAIN`) and aborted blocks.
#[tokio::test]
async fn reports_the_server_transaction_state() {
    let Some(mut s) = session().await else { return };
    let state = async |s: &mut PgSession, sql: &str| match collect(s, sql, 10).await.last() {
        Some(QueryEvent::Done { in_transaction, .. } | QueryEvent::Error { in_transaction, .. }) => *in_transaction,
        other => panic!("{sql}: {other:?}"),
    };
    assert!(!state(&mut s, "select 1").await);
    assert!(state(&mut s, "start transaction").await);
    assert!(state(&mut s, "commit and chain").await);
    assert!(state(&mut s, "select 1 / 0").await, "an aborted block is still open");
    assert!(!state(&mut s, "rollback").await);
    assert!(!state(&mut s, "select 1").await);
}

async fn run_all(s: &mut PgSession, statements: &[&str]) {
    for sql in statements {
        let events = collect(s, sql, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
    }
}

#[tokio::test]
async fn checks_without_running_anything() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "create table if not exists idedb_check (id serial primary key, v int)",
            "insert into idedb_check (v) values (1), (2)",
            "create sequence if not exists idedb_check_seq",
        ],
    )
    .await;
    testing::check_runs_nothing(
        &mut s,
        &[
            "insert into idedb_check (v) values (3) returning *",
            "update idedb_check set v = v + 1",
            "delete from idedb_check",
            "select nextval('idedb_check_seq')",
            "select nextval(pg_get_serial_sequence('idedb_check', 'id'))",
            "truncate idedb_check",
            "drop table idedb_check",
            // Parameters: typed from context, or untyped (not the user's error).
            "select * from idedb_check where id = $1",
            "select $1",
        ],
        "select (select count(*) from idedb_check), (select sum(v) from idedb_check),
                (select last_value from idedb_check_seq), (select is_called from idedb_check_seq),
                (select last_value from idedb_check_id_seq)",
    )
    .await;
}

#[tokio::test]
async fn checks_report_problems_where_postgres_locates_them() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["create table if not exists idedb_check_problems (id int)"]).await;
    testing::check_reports(&mut s, "select from from idedb_check_problems", "syntax error", Some("from idedb")).await;
    testing::check_reports(&mut s, "select * from idedb_missing", "\"idedb_missing\" does not exist", Some("idedb_missing"))
        .await;
    // Multi-byte text before the problem: positions are in chars.
    testing::check_reports(
        &mut s,
        "select 'ñandú', nope from idedb_check_problems",
        "column \"nope\" does not exist",
        Some("nope"),
    )
    .await;
}

#[tokio::test]
async fn checks_and_sets_schema() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "create schema if not exists idedb_check_s",
            "create table if not exists idedb_check_s.only_here (id int)",
            "create table if not exists idedb_check_public (id int)",
        ],
    )
    .await;
    let sql = "select * from only_here";
    assert!(s.check(sql, None).await.unwrap().is_some(), "not on the default search path");
    assert_eq!(s.check(sql, Some("idedb_check_s")).await.unwrap(), None);
    // The scoped search path did not leak into the session.
    assert!(s.check(sql, None).await.unwrap().is_some());
    let events = collect(&mut s, sql, 10).await;
    assert!(matches!(events.last(), Some(QueryEvent::Error { .. })), "{events:?}");

    s.set_schema("idedb_check_s").await.unwrap();
    run_all(&mut s, &[sql, "select count(*) from idedb_check_public"]).await; // public stays reachable
    s.set_schema("public").await.unwrap(); // the default: back to the server's search path
    let events = collect(&mut s, sql, 10).await;
    assert!(matches!(events.last(), Some(QueryEvent::Error { .. })), "{events:?}");
}

/// Values travel as text and are cast to each column's type, so types with
/// no native `Value` variant are edited and read back exactly.
#[tokio::test]
async fn applies_values_of_any_type() {
    use idedb_core::{ApplyOutcome, ColumnValue, RowChange, TableRef};

    let Some(mut s) = session().await else { return };
    for sql in [
        "drop table if exists idedb_apply_types",
        "create table idedb_apply_types (id serial primary key, amount numeric(12,2), doc jsonb, at timestamptz,
            bin bytea, tags int[], flag boolean, created date default '2026-01-01')",
    ] {
        let events = collect(&mut s, sql, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
    }
    let table = TableRef { schema: "public".into(), name: "idedb_apply_types".into() };
    let cv = |column: &str, value: Value| ColumnValue { column: column.into(), value };
    let text = |s: &str| Value::Text(s.into());

    let outcome = s
        .apply(
            &table,
            &[RowChange::Insert {
                values: vec![
                    cv("amount", text("12.5")),
                    cv("doc", text(r#"{"b": 1, "a": [true]}"#)),
                    cv("at", text("2026-09-25 10:00:00+00")),
                    cv("bin", Value::Bytes(vec![0xde, 0xad])),
                    cv("tags", text("{1,2}")),
                    cv("flag", Value::Bool(true)),
                ],
            }],
        )
        .await
        .unwrap();
    let ApplyOutcome::Applied { rows, .. } = outcome else { panic!("{outcome:?}") };
    assert_eq!(
        rows[0].as_ref().unwrap()[1..],
        [
            text("12.50"),
            text(r#"{"a": [true], "b": 1}"#),
            text("2026-09-25 10:00:00+00:00"),
            Value::Bytes(vec![0xde, 0xad]),
            text("{1,2}"),
            Value::Bool(true),
            text("2026-01-01"),
        ]
    );

    let id = rows[0].as_ref().unwrap()[0].clone();
    let outcome = s
        .apply(&table, &[RowChange::Update { key: vec![cv("id", id)], values: vec![cv("amount", text("not a number"))] }])
        .await
        .unwrap();
    let ApplyOutcome::Failed { index: 0, message } = outcome else { panic!("{outcome:?}") };
    assert!(message.contains("invalid input syntax for type numeric"), "{message}");
    testing::assert_usable(&mut s).await;
}

const READ_ONLY: ConnectOptions = ConnectOptions { read_only: true };

/// A read-only session, next to a read-write one that created `table` with
/// one row for it to read.
async fn read_only_sessions(table: &str) -> Option<(PgSession, PgSession)> {
    let mut rw = session().await?;
    run_all(
        &mut rw,
        &[
            &format!("drop table if exists {table}"),
            &format!("create table {table} (id int primary key)"),
            &format!("insert into {table} values (1)"),
        ],
    )
    .await;
    let ro = session_with(READ_ONLY).await?;
    Some((rw, ro))
}

#[tokio::test]
async fn read_only_refuses_writes() {
    let Some((mut rw, mut ro)) = read_only_sessions("idedb_read_only").await else { return };
    testing::read_only_refuses_writes(&mut ro, "insert into idedb_read_only values (2)", "select id from idedb_read_only")
        .await;
    // The driver's own read transactions still work: a paged read, a paused one.
    testing::fetches_on_demand(&mut ro, "select g from generate_series(1, 23) g", 23).await;
    run_all(&mut rw, &["drop table idedb_read_only"]).await;
}

/// The connection that replaces a lost one is read only too.
#[tokio::test]
async fn read_only_survives_reconnecting() {
    let Some((mut rw, mut ro)) = read_only_sessions("idedb_read_only_reconnect").await else { return };
    terminate(&mut ro).await;
    let events = collect(&mut ro, "select 1", 10).await;
    assert!(last_error(&events).0.contains("re-established"), "{events:?}");
    testing::read_only_refuses_writes(
        &mut ro,
        "insert into idedb_read_only_reconnect values (2)",
        "select id from idedb_read_only_reconnect",
    )
    .await;
    run_all(&mut rw, &["drop table idedb_read_only_reconnect"]).await;
}
