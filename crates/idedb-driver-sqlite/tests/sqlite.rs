//! Integration tests against real SQLite files in a temp directory.

use idedb_core::testing::{self, collect, rows};
use idedb_core::{
    Canceller, ColumnInfo, ConnectionParams, Engine, Error, ForeignKey, ObjectKind, QueryEvent, SchemaInfo, Session, SslMode,
    Value,
};
use idedb_driver_sqlite::SqliteSession;
use tempfile::TempDir;

fn params(path: &str) -> ConnectionParams {
    ConnectionParams {
        engine: Engine::Sqlite,
        host: String::new(),
        port: None,
        user: String::new(),
        database: String::new(),
        ssl_mode: SslMode::Disable,
        path: path.to_owned(),
    }
}

/// Keep the returned `TempDir` alive for as long as the session.
async fn session() -> (TempDir, SqliteSession) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let session = SqliteSession::connect(&params(path.to_str().unwrap()), None).await.expect("connect");
    (dir, session)
}

async fn run(session: &mut SqliteSession, sql: &str) {
    let events = collect(session, sql, 100).await;
    assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
}

const COUNT_FOREVER: &str = "with recursive c(x) as (select 1 union all select x + 1 from c) select count(*) from c";

#[tokio::test]
async fn connects_and_reports_server_info() {
    let (dir, session) = session().await;
    let info = session.server_info();
    assert_eq!(info.engine, Engine::Sqlite);
    assert!(info.version.starts_with('3'), "{}", info.version);
    assert_eq!(info.default_schema.as_deref(), Some("main"));
    assert!(dir.path().join("test.db").exists(), "database file is created on connect");
}

#[tokio::test]
async fn rejects_missing_path_and_unreadable_files() {
    let empty = SqliteSession::connect(&params("  "), None).await;
    assert!(matches!(empty, Err(Error::InvalidParams(_))));

    let dir = tempfile::tempdir().unwrap();
    let missing_dir = dir.path().join("nope/test.db");
    let missing = SqliteSession::connect(&params(missing_dir.to_str().unwrap()), None).await;
    assert!(matches!(missing, Err(Error::Connect(_))));

    let not_a_db = dir.path().join("notes.txt");
    std::fs::write(&not_a_db, "this is not a database, just some text that is long enough").unwrap();
    let garbage = SqliteSession::connect(&params(not_a_db.to_str().unwrap()), None).await;
    assert!(matches!(&garbage, Err(Error::Connect(m)) if m.contains("not a database")), "{:?}", garbage.err());
}

#[tokio::test]
async fn streams_in_pages() {
    let (_dir, mut s) = session().await;
    testing::streams_in_pages(
        &mut s,
        "with recursive s(n) as (select 1 union all select n + 1 from s where n < 100000) select n, 'row ' || n from s",
        100_000,
        2000,
    )
    .await;
}

#[tokio::test]
async fn cancels_a_running_statement() {
    let (_dir, s) = session().await;
    testing::cancels_a_running_statement(s, COUNT_FOREVER).await;
}

#[tokio::test]
async fn cancels_between_pages() {
    let (_dir, mut s) = session().await;
    testing::cancels_between_pages(
        &mut s,
        "with recursive s(n) as (select 1 union all select n + 1 from s where n < 10000000) select n from s",
        1000,
    )
    .await;
}

#[tokio::test]
async fn reports_affected_rows_and_errors() {
    let (_dir, mut s) = session().await;
    testing::reports_affected_rows_and_errors(
        &mut s,
        "create table t (id integer)",
        "insert into t values (1), (2), (3)",
        "select * from missing_table",
        "no such table: missing_table",
    )
    .await;

    // DDL after DML must not report the previous statement's count.
    let dropped = collect(&mut s, "create index t_id on t (id)", 10).await;
    assert!(matches!(dropped.as_slice(), [QueryEvent::Done { row_count: 0, .. }]), "{dropped:?}");
    let updated = collect(&mut s, "update t set id = id + 1 where id > 1", 10).await;
    assert!(matches!(updated.as_slice(), [QueryEvent::Done { row_count: 2, .. }]), "{updated:?}");
}

#[tokio::test]
async fn cancel_is_a_no_op_when_idle() {
    let (_dir, mut s) = session().await;
    let canceller = s.canceller();
    canceller.cancel().await.unwrap();
    canceller.cancel().await.unwrap();
    // A stale request must not abort the next statement, short or long.
    testing::assert_usable(&mut s).await;
    let events = collect(&mut s, "with recursive s(n) as (select 1 union all select n + 1 from s where n < 50000) select count(*) from s", 10).await;
    assert_eq!(rows(&events), vec![vec![Value::Int(50000)]], "{events:?}");
}

#[tokio::test]
async fn decodes_values_and_declared_types() {
    let (_dir, mut s) = session().await;
    run(&mut s, "create table typed (a INTEGER, b VARCHAR(10), c blob, d real)").await;
    run(&mut s, "insert into typed values (7, 'héllo', x'00ff', 1.5), (null, null, null, null)").await;

    let events = collect(&mut s, "select a, b, c, d, a + 1 as next, cast(x'ff' as text) as bad_utf8 from typed", 10).await;
    let QueryEvent::Columns { columns } = &events[0] else { panic!("{events:?}") };
    let types: Vec<&str> = columns.iter().map(|c| c.type_name.as_str()).collect();
    assert_eq!(types, ["integer", "varchar(10)", "blob", "real", "", ""]);
    assert_eq!(columns[4].name, "next");

    assert_eq!(
        rows(&events),
        vec![
            vec![
                Value::Int(7),
                Value::Text("héllo".into()),
                Value::Bytes(vec![0, 255]),
                Value::Float(1.5),
                Value::Int(8),
                Value::Bytes(vec![255]),
            ],
            vec![Value::Null, Value::Null, Value::Null, Value::Null, Value::Null, Value::Bytes(vec![255])],
        ]
    );
}

#[tokio::test]
async fn lists_schemas() {
    let (dir, mut s) = session().await;
    let names = |schemas: Vec<SchemaInfo>| schemas.into_iter().map(|s| s.name).collect::<Vec<_>>();
    assert_eq!(names(s.schemas().await.unwrap()), ["main"]);

    let other = dir.path().join("other.db");
    run(&mut s, &format!("attach database '{}' as other", other.display())).await;
    run(&mut s, "create temp table scratch (x)").await;
    let schemas = s.schemas().await.unwrap();
    assert!(schemas.iter().all(|s| !s.is_system));
    assert_eq!(names(schemas), ["main", "temp", "other"]);
}

#[tokio::test]
async fn introspects_tables_views_keys() {
    let (_dir, mut s) = session().await;
    for sql in [
        "create table authors (id integer primary key, name text not null default 'anon')",
        // Composite primary key; the foreign key references authors' PK implicitly.
        "create table books (author_id integer not null, seq integer not null, title varchar(200),
                             primary key (author_id, seq), foreign key (author_id) references authors)",
        "create table reviews (id integer primary key, book_author integer, book_seq integer, body text,
                               foreign key (book_author, book_seq) references books (author_id, seq))",
        "create view long_books as select title from books where length(title) > 100",
    ] {
        run(&mut s, sql).await;
    }

    let model = s.introspect("main").await.unwrap();
    let names: Vec<&str> = model.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["authors", "books", "long_books", "reviews"]);

    let column = |name: &str, type_name: &str, nullable: bool, default: Option<&str>, pk: Option<u16>| ColumnInfo {
        name: name.into(),
        type_name: type_name.into(),
        nullable,
        default: default.map(Into::into),
        primary_key: pk,
        comment: None,
        generated: false,
    };

    let authors = &model.tables[0];
    assert_eq!(authors.kind, ObjectKind::Table);
    assert_eq!(
        authors.columns,
        // A lone INTEGER PRIMARY KEY is the rowid: SQLite assigns it.
        [
            ColumnInfo { generated: true, ..column("id", "integer", true, None, Some(1)) },
            column("name", "text", false, Some("'anon'"), None)
        ]
    );

    let books = &model.tables[1];
    assert_eq!(
        books.columns,
        [
            column("author_id", "integer", false, None, Some(1)),
            column("seq", "integer", false, None, Some(2)),
            column("title", "varchar(200)", true, None, None),
        ]
    );
    assert_eq!(
        books.foreign_keys,
        [ForeignKey {
            name: "fk_books_0".into(),
            columns: vec!["author_id".into()],
            referenced_schema: "main".into(),
            referenced_table: "authors".into(),
            referenced_columns: vec!["id".into()],
        }]
    );

    let view = &model.tables[2];
    assert_eq!(view.kind, ObjectKind::View);
    assert_eq!(view.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["title"]);
    assert!(view.foreign_keys.is_empty());

    let reviews = &model.tables[3];
    assert_eq!(
        reviews.foreign_keys,
        [ForeignKey {
            name: "fk_reviews_0".into(),
            columns: vec!["book_author".into(), "book_seq".into()],
            referenced_schema: "main".into(),
            referenced_table: "books".into(),
            referenced_columns: vec!["author_id".into(), "seq".into()],
        }]
    );

    let missing = s.introspect("nope").await;
    assert!(matches!(missing, Err(Error::Query(_))), "{missing:?}");
}

#[tokio::test]
async fn introspection_survives_a_cancelled_statement() {
    let (_dir, s) = session().await;
    let mut s = testing::cancels_a_running_statement(s, COUNT_FOREVER).await;
    run(&mut s, "create table after_cancel (x)").await;
    let model = s.introspect("main").await.unwrap();
    assert_eq!(model.tables.len(), 1);
}

#[tokio::test]
async fn locates_syntax_errors_in_chars() {
    let (_dir, mut s) = session().await;
    // Multi-byte text before the error proves the offset is in chars, not bytes.
    let events = collect(&mut s, "select 'ñandú' from from t", 10).await;
    let Some(QueryEvent::Error { message, position, .. }) = events.last() else { panic!("{events:?}") };
    assert!(message.contains("syntax error"), "{message}");
    assert!(!message.contains("select"), "message echoes the SQL: {message}");
    assert_eq!(*position, Some(20), "{message}");
}

#[tokio::test]
async fn applies_row_changes() {
    let (_dir, mut s) = session().await;
    testing::applies_row_changes(&mut s, "main").await;
}

/// Generated columns are flagged; a key is the rowid only in a rowid table.
#[tokio::test]
async fn flags_generated_columns() {
    let (_dir, mut s) = session().await;
    run(&mut s, "create table g (id integer primary key, v int, twice int generated always as (v * 2) stored)").await;
    run(&mut s, "create table w (id integer primary key, v int) without rowid").await;
    let model = s.introspect("main").await.unwrap();
    let flags = |table: &str| -> Vec<(String, bool)> {
        let t = model.tables.iter().find(|t| t.name == table).unwrap();
        t.columns.iter().map(|c| (c.name.clone(), c.generated)).collect()
    };
    let f = |n: &str, g: bool| (n.to_owned(), g);
    assert_eq!(flags("g"), [f("id", true), f("v", false), f("twice", true)]);
    assert_eq!(flags("w"), [f("id", false), f("v", false)]);
}

#[tokio::test]
async fn respects_user_transactions() {
    let (_dir, mut s) = session().await;
    testing::respects_user_transactions(&mut s, "main").await;
}

#[tokio::test]
async fn checks_without_running_anything() {
    let (_dir, mut s) = session().await;
    run(&mut s, "create table idedb_check (id integer primary key, v int)").await;
    run(&mut s, "insert into idedb_check (v) values (1), (2)").await;
    testing::check_runs_nothing(
        &mut s,
        &[
            "insert into idedb_check (v) values (3) returning *",
            "update idedb_check set v = v + 1",
            "delete from idedb_check",
            "drop table idedb_check",
            "create table idedb_check_new (id int)",
            // SQLite binds missing parameters as NULL, so these compile too.
            "select * from idedb_check where id = ?1 or v = :v",
        ],
        "select count(*), sum(v), (select count(*) from sqlite_schema) from idedb_check",
    )
    .await;
}

#[tokio::test]
async fn checks_report_problems_where_sqlite_locates_them() {
    let (_dir, mut s) = session().await;
    run(&mut s, "create table idedb_check_problems (id int)").await;
    testing::check_reports(&mut s, "select from from idedb_check_problems", "syntax error", Some("from from")).await;
    // SQLite locates syntax and column problems, not missing tables.
    testing::check_reports(&mut s, "select * from idedb_missing", "no such table: idedb_missing", None).await;
    // Multi-byte text before the problem: the position is in chars.
    testing::check_reports(
        &mut s,
        "select 'ñandú', nope from idedb_check_problems",
        "no such column: nope",
        Some("nope"),
    )
    .await;
}
