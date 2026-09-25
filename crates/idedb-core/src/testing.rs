//! Conformance checks every driver runs against a real server, so all
//! engines behave the same way behind [`Session`]. Enabled with the
//! `testing` feature, for drivers' integration tests only.

use std::time::{Duration, Instant};

use crate::{
    ApplyOutcome, Canceller, ColumnValue, QueryEvent, ROW_NOT_FOUND, Row, RowChange, Session, TableRef, Value,
};

pub async fn collect(session: &mut impl Session, sql: &str, page_size: usize) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    session.execute(sql, page_size, &mut |e| events.push(e)).await;
    events
}

pub fn rows(events: &[QueryEvent]) -> Vec<Row> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Rows { rows } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// `sql` must return exactly `expected` rows.
pub async fn streams_in_pages(session: &mut impl Session, sql: &str, expected: usize, page_size: usize) {
    let started = Instant::now();
    let (mut pages, mut total, mut columns, mut done) = (0, 0, false, None);
    session
        .execute(sql, page_size, &mut |e| match e {
            QueryEvent::Columns { .. } => {
                assert_eq!(pages, 0, "Columns must come before any Rows");
                columns = true;
            }
            QueryEvent::Rows { rows } => {
                assert!(!rows.is_empty() && rows.len() <= page_size, "page of {} rows", rows.len());
                pages += 1;
                total += rows.len();
            }
            QueryEvent::Done { row_count, cancelled, .. } => done = Some((row_count, cancelled)),
            QueryEvent::Error { message, .. } => panic!("{message}"),
        })
        .await;

    assert!(columns, "missing Columns event");
    assert_eq!(total, expected);
    assert_eq!(pages, expected.div_ceil(page_size));
    assert_eq!(done, Some((expected as u64, false)));
    eprintln!("{expected} rows in {pages} pages: {:?}", started.elapsed());
}

/// `long_sql` must run for several seconds unless cancelled.
pub async fn cancels_a_running_statement<S: Session>(mut session: S, long_sql: &'static str) -> S {
    let canceller = session.canceller();
    let started = Instant::now();
    let run = tokio::spawn(async move {
        let events = collect(&mut session, long_sql, 100).await;
        (session, events)
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    canceller.cancel().await.expect("cancel");
    let (mut session, events) = run.await.expect("join");

    assert!(
        matches!(events.last(), Some(QueryEvent::Done { cancelled: true, .. })),
        "expected a cancelled Done, got {events:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "cancel took {:?}", started.elapsed());
    assert_usable(&mut session).await;
    session
}

/// `sql` must return far more than `page_size` rows.
///
/// Spawns a cancel on every page, so it also checks that repeated
/// `cancel()` calls are idempotent: extra requests must not leak into the
/// next statement.
pub async fn cancels_between_pages(session: &mut impl Session, sql: &str, page_size: usize) {
    let canceller = session.canceller();
    let (mut total, mut last) = (0, None);
    session
        .execute(sql, page_size, &mut |e| {
            if let QueryEvent::Rows { rows } = &e {
                total += rows.len();
                // Cancel from inside the stream, as the UI would mid-scroll.
                let c = canceller.clone();
                tokio::spawn(async move { c.cancel().await });
            }
            last = Some(e);
        })
        .await;

    assert!(matches!(last, Some(QueryEvent::Done { cancelled: true, .. })), "{last:?}");
    assert!(total > 0, "no rows before the cancel");
    assert_usable(session).await;
}

/// Statements without a result set report affected rows; failures are
/// `Error` events and leave the session usable.
pub async fn reports_affected_rows_and_errors(
    session: &mut impl Session,
    create_table: &str,
    insert_three_rows: &str,
    bad_statement: &str,
    expected_error_fragment: &str,
) {
    let created = collect(session, create_table, 10).await;
    assert!(matches!(created.as_slice(), [QueryEvent::Done { cancelled: false, .. }]), "{created:?}");

    let inserted = collect(session, insert_three_rows, 10).await;
    assert!(matches!(inserted.as_slice(), [QueryEvent::Done { row_count: 3, .. }]), "{inserted:?}");

    let failed = collect(session, bad_statement, 10).await;
    let Some(QueryEvent::Error { message, .. }) = failed.last() else { panic!("{failed:?}") };
    assert!(message.contains(expected_error_fragment), "unexpected error: {message}");

    assert_usable(session).await;
}

/// Data editor changes through [`Session::apply`]. Creates `idedb_apply`
/// (composite primary key `(a, b)` plus a nullable `note`) in `schema`, which
/// must be where unqualified names resolve.
pub async fn applies_row_changes(session: &mut impl Session, schema: &str) {
    for sql in [
        "drop table if exists idedb_apply",
        "create table idedb_apply (a int not null, b varchar(10) not null, note varchar(50), primary key (a, b))",
        "insert into idedb_apply (a, b, note) values (1, 'x', 'one'), (2, 'y', 'two')",
    ] {
        let events = collect(session, sql, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
    }

    let table = TableRef { schema: schema.into(), name: "idedb_apply".into() };
    let cv = |column: &str, value: Value| ColumnValue { column: column.into(), value };
    let text = |s: &str| Value::Text(s.into());
    let key = |a: i64, b: &str| vec![cv("a", Value::Int(a)), cv("b", text(b))];
    let row = |a: i64, b: &str, note: Value| vec![Value::Int(a), text(b), note];
    let contents = async |session: &mut _| rows(&collect(session, "select a, b, note from idedb_apply order by a", 10).await);

    // Update to NULL, insert with a column left to its default, delete.
    let outcome = session
        .apply(
            &table,
            &[
                RowChange::Update { key: key(1, "x"), values: vec![cv("note", Value::Null)] },
                RowChange::Insert { values: vec![cv("a", Value::Int(3)), cv("b", text("z"))] },
                RowChange::Delete { key: key(2, "y") },
            ],
        )
        .await
        .expect("apply");
    assert_eq!(
        outcome,
        ApplyOutcome::Applied { rows: vec![Some(row(1, "x", Value::Null)), Some(row(3, "z", Value::Null)), None] }
    );
    assert_eq!(contents(session).await, vec![row(1, "x", Value::Null), row(3, "z", Value::Null)]);

    // Editing a key column: the row is found by its old key.
    let outcome = session
        .apply(&table, &[RowChange::Update { key: key(3, "z"), values: vec![cv("b", text("w")), cv("note", text("three"))] }])
        .await
        .expect("apply");
    assert_eq!(outcome, ApplyOutcome::Applied { rows: vec![Some(row(3, "w", text("three")))] });

    // Any failure rolls back the whole batch: a missing row...
    let outcome = session
        .apply(
            &table,
            &[
                RowChange::Update { key: key(1, "x"), values: vec![cv("note", text("changed"))] },
                RowChange::Update { key: key(99, "nope"), values: vec![cv("note", text("lost"))] },
            ],
        )
        .await
        .expect("apply");
    assert_eq!(outcome, ApplyOutcome::Failed { index: 1, message: ROW_NOT_FOUND.into() });

    // ...a server error (duplicate key)...
    let outcome = session
        .apply(
            &table,
            &[RowChange::Delete { key: key(3, "w") }, RowChange::Insert { values: key(1, "x") }],
        )
        .await
        .expect("apply");
    assert!(matches!(&outcome, ApplyOutcome::Failed { index: 1, message } if !message.is_empty()), "{outcome:?}");

    // ...or a delete that matches nothing.
    let outcome = session.apply(&table, &[RowChange::Delete { key: key(42, "none") }]).await.expect("apply");
    assert_eq!(outcome, ApplyOutcome::Failed { index: 0, message: ROW_NOT_FOUND.into() });

    assert_eq!(contents(session).await, vec![row(1, "x", Value::Null), row(3, "w", text("three"))]);
    assert_usable(session).await;
}

pub async fn assert_usable(session: &mut impl Session) {
    let events = collect(session, "select 1", 10).await;
    assert_eq!(rows(&events), vec![vec![Value::Int(1)]], "session unusable: {events:?}");
}
