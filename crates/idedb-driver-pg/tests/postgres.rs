//! Integration tests against a real Postgres.
//!
//! Start one with `docker compose up -d --wait` and run with
//! `IDEDB_PG_URL=postgres://idedb:idedb@localhost:54329/idedb cargo test -p idedb-driver-pg`.
//! Without `IDEDB_PG_URL` these tests are skipped.

use idedb_core::testing::{self, collect, rows};
use idedb_core::{
    ColumnInfo, ConnectionParams, Engine, ForeignKey, ObjectKind, QueryEvent, Session, SslMode, Value,
};
use idedb_driver_pg::PgSession;

async fn session() -> Option<PgSession> {
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
    Some(PgSession::connect(&params, password.as_deref()).await.expect("connect"))
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
            text(r#"{"a":1}"#),
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
            ColumnInfo { name: "tenant".into(), type_name: "integer".into(), nullable: false, default: None, primary_key: Some(1), comment: None },
            ColumnInfo { name: "id".into(), type_name: "bigint".into(), nullable: false, default: None, primary_key: Some(2), comment: None },
            ColumnInfo { name: "note".into(), type_name: "text".into(), nullable: true, default: Some("'n/a'::text".into()), primary_key: None, comment: None },
        ]
    );

    let lines = &model.tables[0];
    assert_eq!(lines.columns[0].default.as_deref(), Some("nextval('idedb_introspect.lines_id_seq'::regclass)"));
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
    let Some(QueryEvent::Error { message, position }) = events.last() else { panic!("{events:?}") };
    assert!(message.contains("syntax error"), "{message}");
    assert_eq!(*position, Some(20), "{message}");
}

#[tokio::test]
async fn applies_row_changes() {
    let Some(mut s) = session().await else { return };
    testing::applies_row_changes(&mut s, "public").await;
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
    let ApplyOutcome::Applied { rows } = outcome else { panic!("{outcome:?}") };
    assert_eq!(
        rows[0].as_ref().unwrap()[1..],
        [
            text("12.50"),
            text(r#"{"a":[true],"b":1}"#),
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
