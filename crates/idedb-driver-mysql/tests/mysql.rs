//! Integration tests against a real MySQL.
//!
//! Start one with `docker compose up -d --wait` and run with
//! `IDEDB_MYSQL_URL=mysql://idedb:idedb@localhost:33069/idedb cargo test -p idedb-driver-mysql`.
//! Without `IDEDB_MYSQL_URL` these tests are skipped.

use idedb_core::testing::{self, collect, rows};
use idedb_core::{ConnectionParams, Engine, ObjectKind, QueryEvent, Session, SslMode, Value};
use idedb_driver_mysql::MySqlSession;
use mysql_async::Opts;

/// name, type, nullable, primary key position, comment.
type ColumnSummary<'a> = (&'a str, &'a str, bool, Option<u16>, Option<&'a str>);

async fn session_with(ssl_mode: SslMode) -> Option<MySqlSession> {
    let Ok(url) = std::env::var("IDEDB_MYSQL_URL") else {
        eprintln!("IDEDB_MYSQL_URL not set, skipping");
        return None;
    };
    let opts = Opts::from_url(&url).expect("valid IDEDB_MYSQL_URL");
    let params = ConnectionParams {
        engine: Engine::Mysql,
        host: opts.ip_or_hostname().to_owned(),
        port: Some(opts.tcp_port()),
        user: opts.user().unwrap_or_default().to_owned(),
        database: opts.db_name().unwrap_or_default().to_owned(),
        ssl_mode,
        path: String::new(),
    };
    Some(
        MySqlSession::connect(&params, opts.pass())
            .await
            .expect("connect"),
    )
}

async fn session() -> Option<MySqlSession> {
    session_with(SslMode::Prefer).await
}

/// Runs setup statements one by one, failing loudly on any error.
async fn run_all(session: &mut MySqlSession, statements: &[&str]) {
    for sql in statements {
        let events = collect(session, sql, 100).await;
        assert!(
            matches!(
                events.last(),
                Some(QueryEvent::Done {
                    cancelled: false,
                    ..
                })
            ),
            "{sql}: {events:?}"
        );
    }
}

#[tokio::test]
async fn connects_with_every_ssl_mode_and_reports_server_info() {
    for mode in [SslMode::Disable, SslMode::Prefer, SslMode::Require] {
        let Some(mut s) = session_with(mode).await else {
            return;
        };
        let server = s.server_info().clone();
        assert_eq!(server.engine, Engine::Mysql);
        assert!(server.version.starts_with('8'), "{server:?}");
        assert_eq!(server.default_schema.as_deref(), Some("idedb"));
        testing::assert_usable(&mut s).await;
    }
}

#[tokio::test]
async fn streams_rows_in_pages() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["set session cte_max_recursion_depth = 1000000"]).await;
    testing::streams_in_pages(
        &mut s,
        "with recursive seq(n) as (select 1 union all select n + 1 from seq where n < 100000)
         select n, md5(n) from seq",
        100_000,
        2000,
    )
    .await;
}

#[tokio::test]
async fn cancels_a_running_statement() {
    let Some(s) = session().await else { return };
    testing::cancels_a_running_statement(s, "select sleep(30)").await;
}

/// Unlike `SLEEP()`, a busy statement fails with ER_QUERY_INTERRUPTED when killed.
#[tokio::test]
async fn cancels_a_busy_statement() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["set session cte_max_recursion_depth = 100000000"]).await;
    testing::cancels_a_running_statement(
        s,
        "with recursive seq(n) as (select 1 union all select n + 1 from seq where n < 100000000)
         select count(*) from seq",
    )
    .await;
}

#[tokio::test]
async fn cancels_between_pages() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["set session cte_max_recursion_depth = 10000000"]).await;
    testing::cancels_between_pages(
        &mut s,
        "with recursive seq(n) as (select 1 union all select n + 1 from seq where n < 5000000)
         select n, md5(n) from seq",
        1000,
    )
    .await;
}

#[tokio::test]
async fn reports_affected_rows_and_errors() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["drop table if exists idedb_affected"]).await;
    testing::reports_affected_rows_and_errors(
        &mut s,
        "create table idedb_affected (id int)",
        "insert into idedb_affected values (1), (2), (3)",
        "select * from idedb_missing_table",
        "ERROR 1146 (42S02): Table 'idedb.idedb_missing_table' doesn't exist",
    )
    .await;
}

#[tokio::test]
async fn decodes_common_types() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "drop table if exists idedb_types",
            "create table idedb_types (
                big_u bigint unsigned, small smallint, price decimal(10, 2), ratio double,
                label varchar(20), doc json, raw varbinary(4), payload blob, flags bit(8),
                size enum('s', 'm'), at datetime(3), body text
            )",
            "insert into idedb_types values (
                18446744073709551615, -7, 12345.60, 1.5, 'héllo', '{\"a\": 1}', x'00ff', x'0102',
                b'00000101', 'm', '2026-09-25 10:11:12.500', null
            )",
        ],
    )
    .await;

    let events = collect(&mut s, "select * from idedb_types", 10).await;
    let QueryEvent::Columns { columns } = &events[0] else {
        panic!("{events:?}")
    };
    let types: Vec<&str> = columns.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(
        types,
        [
            "bigint unsigned",
            "smallint",
            "decimal",
            "double",
            "varchar",
            "json",
            "varbinary",
            "blob",
            "bit",
            "enum",
            "datetime",
            "text"
        ]
    );

    let text = |s: &str| Value::Text(s.into());
    assert_eq!(
        rows(&events),
        vec![vec![
            text("18446744073709551615"),
            Value::Int(-7),
            text("12345.60"),
            Value::Float(1.5),
            text("héllo"),
            text(r#"{"a": 1}"#),
            Value::Bytes(vec![0, 255]),
            Value::Bytes(vec![1, 2]),
            Value::Bytes(vec![5]),
            text("m"),
            text("2026-09-25 10:11:12.500"),
            Value::Null,
        ]]
    );

    let literals = collect(&mut s, "select 1, -5, 1.5e0, x'00ff', null", 10).await;
    assert_eq!(
        rows(&literals),
        vec![vec![
            Value::Int(1),
            Value::Int(-5),
            Value::Float(1.5),
            Value::Bytes(vec![0, 255]),
            Value::Null
        ]]
    );
}

#[tokio::test]
async fn introspects_schemas_tables_and_keys() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "drop view if exists idedb_intro_view",
            "drop table if exists idedb_intro_child",
            "drop table if exists idedb_intro_parent",
            "create table idedb_intro_parent (
                a int not null, b varchar(20) not null, note text comment 'free text',
                primary key (b, a)
            ) comment 'parents'",
            "create table idedb_intro_child (
                id bigint unsigned auto_increment primary key,
                pa int not null, pb varchar(20) not null,
                created_at datetime default current_timestamp,
                constraint fk_child_parent foreign key (pb, pa) references idedb_intro_parent (b, a)
            )",
            "create view idedb_intro_view as select id from idedb_intro_child",
        ],
    )
    .await;

    let schemas = s.schemas().await.unwrap();
    let find = |name: &str| {
        schemas
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{schemas:?}"))
    };
    assert!(!find("idedb").is_system);
    // SCHEMATA only lists what the user may see, so `mysql` may be absent.
    assert!(find("information_schema").is_system);
    assert!(find("performance_schema").is_system);
    assert!(
        !schemas[0].is_system,
        "user databases come first: {schemas:?}"
    );

    let model = s.introspect("idedb").await.unwrap();
    let names: Vec<&str> = model.tables.iter().map(|t| t.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(names, sorted, "tables are sorted by name");
    let table = |name: &str| {
        model
            .tables
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{names:?}"))
    };

    let parent = table("idedb_intro_parent");
    assert_eq!(parent.kind, ObjectKind::Table);
    assert_eq!(parent.comment.as_deref(), Some("parents"));
    let columns: Vec<ColumnSummary> = parent
        .columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.type_name.as_str(),
                c.nullable,
                c.primary_key,
                c.comment.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        columns,
        [
            ("a", "int", false, Some(2), None),
            ("b", "varchar(20)", false, Some(1), None),
            ("note", "text", true, None, Some("free text")),
        ]
    );
    assert!(parent.foreign_keys.is_empty());

    let child = table("idedb_intro_child");
    assert_eq!(child.columns[0].type_name, "bigint unsigned");
    assert_eq!(child.columns[0].primary_key, Some(1));
    assert_eq!(
        child.columns[3].default.as_deref(),
        Some("CURRENT_TIMESTAMP")
    );
    assert_eq!(child.foreign_keys.len(), 1);
    let fk = &child.foreign_keys[0];
    assert_eq!(fk.name, "fk_child_parent");
    assert_eq!(fk.columns, ["pb", "pa"]);
    assert_eq!(fk.referenced_schema, "idedb");
    assert_eq!(fk.referenced_table, "idedb_intro_parent");
    assert_eq!(fk.referenced_columns, ["b", "a"]);

    let view = table("idedb_intro_view");
    assert_eq!(view.kind, ObjectKind::View);
    assert_eq!(view.comment, None);
    assert_eq!(view.columns.len(), 1);
}

#[tokio::test]
async fn applies_row_changes() {
    let Some(mut s) = session().await else { return };
    testing::applies_row_changes(&mut s, "idedb").await;
}

/// MySQL has no RETURNING: an inserted row is read back through the
/// generated AUTO_INCREMENT id, with the server's defaults filled in.
#[tokio::test]
async fn reads_back_auto_increment_inserts() {
    use idedb_core::{ApplyOutcome, ColumnValue, RowChange, TableRef};

    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "drop table if exists idedb_apply_auto",
            "create table idedb_apply_auto (id bigint unsigned auto_increment primary key, name varchar(20) not null,
                born date default '2026-01-01', price decimal(8,2) default 1.5)",
        ],
    )
    .await;
    let table = TableRef { schema: "idedb".into(), name: "idedb_apply_auto".into() };
    let name = |n: &str| ColumnValue { column: "name".into(), value: Value::Text(n.into()) };

    let outcome = s
        .apply(&table, &[RowChange::Insert { values: vec![name("first")] }, RowChange::Insert { values: vec![name("second")] }])
        .await
        .unwrap();
    let text = |s: &str| Value::Text(s.into());
    assert_eq!(
        outcome,
        ApplyOutcome::Applied {
            rows: vec![
                Some(vec![Value::Int(1), text("first"), text("2026-01-01"), text("1.50")]),
                Some(vec![Value::Int(2), text("second"), text("2026-01-01"), text("1.50")]),
            ]
        }
    );
}

#[tokio::test]
async fn checks_without_running_anything() {
    let Some(mut s) = session().await else { return };
    run_all(
        &mut s,
        &[
            "create table if not exists idedb_check (id bigint auto_increment primary key, v int)",
            "insert into idedb_check (v) values (1), (2)",
        ],
    )
    .await;
    testing::check_runs_nothing(
        &mut s,
        &[
            "insert into idedb_check (v) values (3)",
            "update idedb_check set v = v + 1",
            "delete from idedb_check",
            "truncate table idedb_check",
            "drop table idedb_check",
            "select * from idedb_check where id = ?",
            // Statements MySQL cannot prepare are skipped, not run.
            "lock tables idedb_check write",
        ],
        "select count(*), sum(v) from idedb_check",
    )
    .await;
}

#[tokio::test]
async fn checks_report_problems_near_where_mysql_says() {
    let Some(mut s) = session().await else { return };
    run_all(&mut s, &["create table if not exists idedb_check_problems (id int)"]).await;
    testing::check_reports(&mut s, "select from from idedb_check_problems", "1064", Some("from from")).await;
    // On a later line, with multi-byte text before it: the position is in chars.
    testing::check_reports(
        &mut s,
        "select 'ñandú',\n  2 frm idedb_check_problems",
        "1064",
        Some("idedb_check_problems"),
    )
    .await;
    // Name errors carry no location in MySQL.
    testing::check_reports(&mut s, "select * from idedb_missing", "idedb_missing' doesn't exist", None).await;
    testing::check_reports(&mut s, "select nope from idedb_check_problems", "Unknown column 'nope'", None).await;
}

#[tokio::test]
async fn checks_and_sets_schema() {
    let Some(mut s) = session().await else { return };
    let sql = "select * from customers";
    // `customers` lives in the seeded `shop` database, not in `idedb`.
    assert!(s.check(sql, None).await.unwrap().is_some());
    assert_eq!(s.check(sql, Some("shop")).await.unwrap(), None);
    assert!(s.check(sql, Some("idedb")).await.unwrap().is_some());

    s.set_schema("shop").await.unwrap();
    run_all(&mut s, &["select count(*) from customers"]).await;
    s.set_schema("idedb").await.unwrap();
    let events = collect(&mut s, sql, 10).await;
    assert!(matches!(events.last(), Some(QueryEvent::Error { .. })), "{events:?}");
}
