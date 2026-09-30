//! Conformance checks every driver runs against a real server, so all
//! engines behave the same way behind [`Session`]. Enabled with the
//! `testing` feature, for drivers' integration tests only.

use std::time::{Duration, Instant};

use crate::{
    ApplyOutcome, Canceller, ColumnValue, Fetch, NO_OPEN_RESULT, QueryEvent, ROW_NOT_FOUND, Row, RowChange, Session,
    TableRef, Value,
};

pub async fn collect(session: &mut impl Session, sql: &str, page_size: usize) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    session.execute(sql, Fetch::all(page_size), &mut |e| events.push(e)).await;
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
        .execute(sql, Fetch::all(page_size), &mut |e| match e {
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
        .execute(sql, Fetch::all(page_size), &mut |e| {
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
        ApplyOutcome::Applied {
            rows: vec![Some(row(1, "x", Value::Null)), Some(row(3, "z", Value::Null)), None],
            in_transaction: false,
        }
    );
    assert_eq!(contents(session).await, vec![row(1, "x", Value::Null), row(3, "z", Value::Null)]);

    // Editing a key column: the row is found by its old key.
    let outcome = session
        .apply(&table, &[RowChange::Update { key: key(3, "z"), values: vec![cv("b", text("w")), cv("note", text("three"))] }])
        .await
        .expect("apply");
    assert_eq!(
        outcome,
        ApplyOutcome::Applied { rows: vec![Some(row(3, "w", text("three")))], in_transaction: false }
    );

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

/// A transaction the user opened is never committed or rolled back behind
/// the user's back: not by a paged read, not by a cancel between pages, not
/// by the data editor or a check. Creates `idedb_user_tx` in `schema`, which
/// must be where unqualified names resolve.
pub async fn respects_user_transactions(session: &mut impl Session, schema: &str) {
    for sql in [
        "drop table if exists idedb_user_tx",
        "create table idedb_user_tx (id int primary key, v int not null)",
        "insert into idedb_user_tx (id, v) values (1, 0), (2, 0), (3, 0), (4, 0), (5, 0)",
    ] {
        let events = collect(session, sql, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
    }
    let changed = async |session: &mut _| {
        rows(&collect(session, "select count(*) from idedb_user_tx where v <> 0", 10).await)
    };
    let unchanged = vec![vec![Value::Int(0)]];
    let expect_tx = async |session: &mut _, sql: &str, open: bool| {
        let events = collect(session, sql, 2).await;
        match events.last() {
            Some(QueryEvent::Done { in_transaction, .. }) => {
                assert_eq!(*in_transaction, open, "{sql}: wrong transaction state in {events:?}")
            }
            other => panic!("{sql}: {other:?}"),
        }
        events
    };

    // A paged read (three pages) inside the user's transaction commits nothing.
    expect_tx(session, "select v from idedb_user_tx", false).await;
    expect_tx(session, "begin", true).await;
    expect_tx(session, "update idedb_user_tx set v = 1", true).await;
    let read = expect_tx(session, "select id from idedb_user_tx order by id", true).await;
    assert_eq!(rows(&read).len(), 5, "{read:?}");
    expect_tx(session, "rollback", false).await;
    assert_eq!(changed(session).await, unchanged, "a paged read committed the user's transaction");

    // Nor does a cancel between pages. (With five rows the read may finish
    // before the cancel lands; `cancels_between_pages` covers interrupting
    // it. Either way the user's transaction must still be open.)
    expect_tx(session, "begin", true).await;
    expect_tx(session, "update idedb_user_tx set v = 2", true).await;
    let canceller = session.canceller();
    let mut last = None;
    session
        .execute("select id from idedb_user_tx order by id", Fetch::all(2), &mut |e| {
            if matches!(e, QueryEvent::Rows { .. }) {
                let c = canceller.clone();
                tokio::spawn(async move { c.cancel().await });
            }
            last = Some(e);
        })
        .await;
    assert!(
        matches!(last, Some(QueryEvent::Done { in_transaction: true, .. })),
        "a cancel between pages must leave the transaction open: {last:?}"
    );
    collect(session, "rollback", 10).await;
    assert_eq!(changed(session).await, unchanged, "a cancelled read committed the user's transaction");

    // Data editor changes inside the user's transaction stay uncommitted...
    let table = TableRef { schema: schema.into(), name: "idedb_user_tx".into() };
    let id = |id: i64| vec![ColumnValue { column: "id".into(), value: Value::Int(id) }];
    let set_v = |v: i64| vec![ColumnValue { column: "v".into(), value: Value::Int(v) }];
    expect_tx(session, "begin", true).await;
    let outcome = session
        .apply(&table, &[RowChange::Update { key: id(1), values: set_v(3) }])
        .await
        .expect("apply");
    assert!(matches!(outcome, ApplyOutcome::Applied { in_transaction: true, .. }), "{outcome:?}");
    // ...a failed batch undoes only itself and leaves the transaction open...
    expect_tx(session, "update idedb_user_tx set v = 4 where id = 2", true).await;
    let outcome = session
        .apply(
            &table,
            &[
                RowChange::Update { key: id(3), values: set_v(5) },
                RowChange::Update { key: id(99), values: set_v(5) },
            ],
        )
        .await
        .expect("apply");
    assert!(matches!(outcome, ApplyOutcome::Failed { index: 1, .. }), "{outcome:?}");
    let inside = rows(&expect_tx(session, "select id, v from idedb_user_tx where v <> 0 order by id", true).await);
    assert_eq!(
        inside,
        vec![vec![Value::Int(1), Value::Int(3)], vec![Value::Int(2), Value::Int(4)]],
        "a failed batch must undo only its own changes"
    );
    // ...and so does a check.
    let problem = session.check("select nope from idedb_user_tx", None).await.expect("check");
    assert!(problem.is_some(), "the check should report the unknown column");
    expect_tx(session, "select 1", true).await;
    // The user's ROLLBACK undoes all of it.
    expect_tx(session, "rollback", false).await;
    assert_eq!(changed(session).await, unchanged, "the data editor committed the user's transaction");

    // Outside a transaction the data editor still commits.
    let outcome = session
        .apply(&table, &[RowChange::Update { key: id(1), values: set_v(6) }])
        .await
        .expect("apply");
    assert!(matches!(outcome, ApplyOutcome::Applied { in_transaction: false, .. }), "{outcome:?}");
    expect_tx(session, "select 1", false).await;
    assert_eq!(changed(session).await, vec![vec![Value::Int(1)]]);
    assert_usable(session).await;
}

/// Checking is validation only: `statements` must all be valid, and after
/// checking them `probe` (a query over whatever they would change: row
/// counts, sequence values, tables) returns exactly what it did before.
pub async fn check_runs_nothing(session: &mut impl Session, statements: &[&str], probe: &str) {
    let before = collect(session, probe, 100).await;
    assert!(matches!(before.last(), Some(QueryEvent::Done { .. })), "probe failed: {before:?}");
    for sql in statements {
        let problem = session.check(sql, None).await.expect("check");
        assert_eq!(problem, None, "{sql} should check clean");
    }
    assert_eq!(rows(&collect(session, probe, 100).await), rows(&before), "checking ran a statement");
    assert_usable(session).await;
}

/// `sql` is invalid: checking it reports a problem mentioning `fragment`,
/// located at the first occurrence of `at` in `sql` (`None` when the engine
/// cannot locate it), and leaves the session usable.
pub async fn check_reports(session: &mut impl Session, sql: &str, fragment: &str, at: Option<&str>) {
    let problem = session
        .check(sql, None)
        .await
        .expect("check")
        .unwrap_or_else(|| panic!("{sql} should not check clean"));
    assert!(problem.message.contains(fragment), "{sql}: unexpected problem {problem:?}");
    let expected = at.map(|needle| {
        let byte = sql.find(needle).unwrap_or_else(|| panic!("{needle} not in {sql}"));
        sql[..byte].chars().count() as u32
    });
    assert_eq!(problem.position, expected, "{sql}: wrong position in {problem:?}");
    assert_usable(session).await;
}

/// Runs `sql` reading only its first `limit` rows.
pub async fn collect_first(session: &mut impl Session, sql: &str, limit: usize, page_size: usize) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    session.execute(sql, Fetch::first(limit, page_size), &mut |e| events.push(e)).await;
    events
}

/// Continues the open result: `limit` more rows, or all of them.
pub async fn collect_more(session: &mut impl Session, limit: Option<usize>, page_size: usize) -> Vec<QueryEvent> {
    let mut events = Vec::new();
    let fetch = Fetch { page_size, limit };
    session.fetch_more(fetch, &mut |e| events.push(e)).await;
    events
}

/// `(row_count, has_more, in_transaction)` of the final `Done`; panics otherwise.
pub fn done(events: &[QueryEvent]) -> (u64, bool, bool) {
    match events.last() {
        Some(QueryEvent::Done { row_count, has_more, in_transaction, .. }) => (*row_count, *has_more, *in_transaction),
        other => panic!("expected Done, got {other:?} in {events:?}"),
    }
}

fn assert_nothing_open(events: &[QueryEvent]) {
    assert!(
        matches!(events, [QueryEvent::Error { message, .. }] if message == NO_OPEN_RESULT),
        "expected the no-open-result error, got {events:?}"
    );
}

/// Reading a result a bit at a time yields exactly the rows of reading it
/// whole. `sql` must return exactly `total` rows (more than 8) in a stable
/// order.
pub async fn fetches_on_demand(session: &mut impl Session, sql: &str, total: usize) {
    assert!(total > 8, "fetches_on_demand needs more than 8 rows");
    let whole = rows(&collect(session, sql, 1000).await);
    assert_eq!(whole.len(), total);

    // The first five rows, in pages of two; the rest stays open.
    let first = collect_first(session, sql, 5, 2).await;
    assert!(matches!(first.first(), Some(QueryEvent::Columns { .. })), "{first:?}");
    for event in &first {
        if let QueryEvent::Rows { rows } = event {
            assert!(!rows.is_empty() && rows.len() <= 2, "page of {} rows", rows.len());
        }
    }
    assert_eq!(done(&first), (5, true, false));

    // Three more, then the rest; continuing sends no columns again.
    let more = collect_more(session, Some(3), 2).await;
    assert!(!more.iter().any(|e| matches!(e, QueryEvent::Columns { .. })), "{more:?}");
    assert_eq!(done(&more), (3, true, false));
    let rest = collect_more(session, None, 1000).await;
    assert_eq!(done(&rest), ((total - 8) as u64, false, false));

    let pieced: Vec<Row> = [rows(&first), rows(&more), rows(&rest)].concat();
    assert_eq!(pieced, whole, "reading in pieces must yield the same rows");
    assert_nothing_open(&collect_more(session, None, 10).await);

    // A result that ends exactly at the limit has nothing more to fetch.
    let exact = collect_first(session, sql, total, 1000).await;
    assert_eq!(done(&exact), (total as u64, false, false));
    assert_eq!(rows(&exact), whole);
    assert_nothing_open(&collect_more(session, None, 10).await);
    assert_usable(session).await;
}

/// An open result is released by `close_result` and by anything else the
/// session does. `sql` must return more than 2 rows.
pub async fn closes_open_results(session: &mut impl Session, sql: &str) {
    let paused = |events: &[QueryEvent]| assert!(done(events).1, "expected an open result: {events:?}");

    paused(&collect_first(session, sql, 2, 10).await);
    session.close_result().await;
    assert_nothing_open(&collect_more(session, None, 10).await);
    // Closing twice, or with nothing open, is fine.
    session.close_result().await;
    assert_usable(session).await;

    // Another statement closes it cleanly and runs normally.
    paused(&collect_first(session, sql, 2, 10).await);
    assert_eq!(rows(&collect(session, "select 1", 10).await), vec![vec![Value::Int(1)]]);
    assert_nothing_open(&collect_more(session, None, 10).await);

    // So does checking a statement.
    paused(&collect_first(session, sql, 2, 10).await);
    assert_eq!(session.check("select 1", None).await.expect("check"), None);
    assert_nothing_open(&collect_more(session, None, 10).await);
    assert_usable(session).await;
}

/// Cancelling a fetch of the rest stops it and leaves the session usable.
/// `sql` must return far more rows than one fetch reads quickly (so the
/// cancel lands while it runs).
pub async fn cancels_fetch_more(session: &mut impl Session, sql: &str) {
    let first = collect_first(session, sql, 10, 10).await;
    assert!(done(&first).1, "expected an open result: {first:?}");

    let canceller = session.canceller();
    let (mut fetched, mut last) = (0, None);
    session
        .fetch_more(Fetch::all(100), &mut |e| {
            if let QueryEvent::Rows { rows } = &e {
                fetched += rows.len();
                let c = canceller.clone();
                tokio::spawn(async move { c.cancel().await });
            }
            last = Some(e);
        })
        .await;
    assert!(matches!(last, Some(QueryEvent::Done { cancelled: true, .. })), "{last:?}");
    assert!(fetched > 0, "no rows before the cancel");
    assert_usable(session).await;
}

/// An open result never becomes a transaction of its own: a paused read
/// neither swallows nor commits what the user runs around it, and closing it
/// leaves the user's transaction as it was. Creates `idedb_open_tx` in
/// `schema`, which must be where unqualified names resolve.
pub async fn open_results_respect_user_transactions(session: &mut impl Session, schema: &str) {
    for sql in [
        "drop table if exists idedb_open_tx",
        "create table idedb_open_tx (id int primary key, v int not null)",
        "insert into idedb_open_tx (id, v) values (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0)",
    ] {
        let events = collect(session, sql, 10).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{sql}: {events:?}");
    }
    let read = "select id from idedb_open_tx order by id";
    let changed = async |session: &mut _| {
        rows(&collect(session, "select count(*) from idedb_open_tx where v <> 0", 10).await)
    };
    let tx = async |session: &mut _, sql: &str| done(&collect(session, sql, 10).await).2;
    // With no transaction open a ROLLBACK undoes nothing; SQLite even calls
    // it an error, the others a warning.
    let rollback_nothing = async |session: &mut _| {
        let events = collect(session, "rollback", 10).await;
        assert!(
            matches!(
                events.last(),
                Some(QueryEvent::Done { in_transaction: false, .. } | QueryEvent::Error { in_transaction: false, .. })
            ),
            "{events:?}"
        );
    };

    // Outside a transaction, a paused read is none the user can see...
    assert_eq!(done(&collect_first(session, read, 3, 2).await), (3, true, false));
    // ...and a statement after it commits on its own: the ROLLBACK below
    // must find nothing to undo.
    assert!(!tx(session, "update idedb_open_tx set v = 1 where id = 1").await);
    rollback_nothing(session).await;
    assert_eq!(changed(session).await, vec![vec![Value::Int(1)]], "the read swallowed the update");

    // The data editor commits too.
    assert!(done(&collect_first(session, read, 3, 2).await).1);
    let table = TableRef { schema: schema.into(), name: "idedb_open_tx".into() };
    let id = |id: i64| vec![ColumnValue { column: "id".into(), value: Value::Int(id) }];
    let set_v = |v: i64| vec![ColumnValue { column: "v".into(), value: Value::Int(v) }];
    let outcome = session.apply(&table, &[RowChange::Update { key: id(2), values: set_v(1) }]).await.expect("apply");
    assert!(matches!(outcome, ApplyOutcome::Applied { in_transaction: false, .. }), "{outcome:?}");
    rollback_nothing(session).await;
    assert_eq!(changed(session).await, vec![vec![Value::Int(2)]], "the read swallowed the data editor's change");
    assert!(!tx(session, "update idedb_open_tx set v = 0").await);

    // Inside the user's transaction, paging and closing commit nothing.
    assert!(tx(session, "begin").await);
    assert!(tx(session, "update idedb_open_tx set v = 5 where id = 3").await);
    assert_eq!(done(&collect_first(session, read, 3, 2).await), (3, true, true));
    assert_eq!(done(&collect_more(session, Some(3), 2).await), (3, true, true));
    session.close_result().await;
    assert!(tx(session, "select 1").await, "closing the result ended the transaction");
    assert!(!tx(session, "rollback").await);
    assert_eq!(changed(session).await, vec![vec![Value::Int(0)]], "paging committed the user's transaction");

    // A statement that closes the open result runs inside the transaction.
    assert!(tx(session, "begin").await);
    assert!(tx(session, "update idedb_open_tx set v = 6 where id = 4").await);
    assert!(done(&collect_first(session, read, 3, 2).await).1);
    assert!(!tx(session, "rollback").await);
    assert_eq!(changed(session).await, vec![vec![Value::Int(0)]], "closing the read committed the user's transaction");

    // Cancelling a fetch inside the transaction leaves it open and usable.
    assert!(tx(session, "begin").await);
    assert!(tx(session, "update idedb_open_tx set v = 7 where id = 5").await);
    assert!(done(&collect_first(session, read, 2, 1).await).1);
    let canceller = session.canceller();
    session
        .fetch_more(Fetch::all(1), &mut |e| {
            if matches!(e, QueryEvent::Rows { .. }) {
                let c = canceller.clone();
                tokio::spawn(async move { c.cancel().await });
            }
        })
        .await;
    let inside = collect(session, "select count(*) from idedb_open_tx where v <> 0", 10).await;
    assert!(done(&inside).2, "a cancelled fetch ended the user's transaction: {inside:?}");
    assert_eq!(rows(&inside), vec![vec![Value::Int(1)]], "the transaction lost its update");
    assert!(!tx(session, "rollback").await);
    assert_eq!(changed(session).await, vec![vec![Value::Int(0)]]);
    assert_usable(session).await;
}

/// A read-only session ([`ConnectOptions::read_only`](crate::ConnectOptions::read_only))
/// refuses `write_sql` with an `Error`, on its own and inside a transaction
/// the user opened, and stays usable: `read_sql` (a query over what the
/// write would change) runs before and after, with the same rows.
/// `write_sql` must write to a table that exists and is not temporary: read
/// only still lets some engines write to the session's own temporary tables.
pub async fn read_only_refuses_writes<S: Session>(session: &mut S, write_sql: &str, read_sql: &str) {
    let read = async |session: &mut S| {
        let events = collect(session, read_sql, 100).await;
        assert!(matches!(events.last(), Some(QueryEvent::Done { .. })), "{read_sql}: {events:?}");
        rows(&events)
    };
    let write = async |session: &mut S| {
        let events = collect(session, write_sql, 100).await;
        match events.last() {
            Some(QueryEvent::Error { in_transaction, .. }) => *in_transaction,
            _ => panic!("a read-only session ran {write_sql}: {events:?}"),
        }
    };
    let before = read(session).await;

    assert!(!write(session).await, "a refused write left a transaction open");
    assert_eq!(read(session).await, before, "a refused write changed the data");

    // A transaction the user opens is read only too.
    let begun = collect(session, "begin", 10).await;
    assert!(matches!(begun.last(), Some(QueryEvent::Done { in_transaction: true, .. })), "{begun:?}");
    write(session).await;
    // The engine may already have ended the transaction; either way none is
    // left open.
    let ended = collect(session, "rollback", 10).await;
    assert!(
        matches!(
            ended.last(),
            Some(QueryEvent::Done { in_transaction: false, .. } | QueryEvent::Error { in_transaction: false, .. })
        ),
        "{ended:?}"
    );
    assert_eq!(read(session).await, before, "a refused write inside a transaction changed the data");
    assert_usable(session).await;
}

pub async fn assert_usable(session: &mut impl Session) {
    let events = collect(session, "select 1", 10).await;
    assert_eq!(rows(&events), vec![vec![Value::Int(1)]], "session unusable: {events:?}");
}
