use std::future::Future;

use crate::{
    ApplyOutcome, Fetch, QueryEvent, Result, RowChange, SchemaInfo, SchemaModel, ServerInfo, SqlProblem, TableRef,
};

/// An open connection to one database server, implemented by each driver.
///
/// A session runs one statement at a time; callers serialize access. The app
/// dispatches over the concrete drivers with an enum, so the trait uses
/// `async fn`-style methods rather than being object safe.
///
/// A session holds at most one open result: the rows a statement had left
/// when it paused at its fetch limit. Anything else the session does
/// (another statement, `apply`, `check`, `set_schema`, introspection) closes
/// it first, so an open result never outlives what the user sees it belong to.
pub trait Session: Send + 'static {
    type Canceller: Canceller;

    /// Handle that cancels whatever this session is running, usable while the
    /// session itself is busy.
    fn canceller(&self) -> Self::Canceller;

    /// Captured when the session connected.
    fn server_info(&self) -> &ServerInfo;

    /// Runs one statement, reporting through `emit`, in order:
    /// `Columns` (only for statements that return rows), `Rows` in pages of
    /// at most `fetch.page_size` rows, then exactly one `Done` or `Error`.
    ///
    /// With a `fetch.limit`, reading pauses after that many rows: `Done`
    /// reports `has_more: true` and the rest stays open for
    /// [`fetch_more`](Self::fetch_more). Holding a result open never makes a
    /// transaction the user can see: `in_transaction` is about the user's own.
    ///
    /// Never fails itself: every outcome, including a cancellation
    /// (`Done { cancelled: true }`), is an event. The session stays usable
    /// afterwards.
    ///
    /// Never begins, commits or rolls back a transaction on its own: one the
    /// user opened stays exactly as the user's statements leave it, and the
    /// final event reports whether one is open.
    fn execute(
        &mut self,
        sql: &str,
        fetch: Fetch,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> impl Future<Output = ()> + Send;

    /// Continues the open result: `Rows` pages, then `Done` (`row_count`
    /// counts this call's rows; `has_more` says whether the result is still
    /// open) or `Error`. Without an open result it emits an `Error` with
    /// [`NO_OPEN_RESULT`](crate::NO_OPEN_RESULT). Cancellable like `execute`:
    /// a cancelled fetch reports `cancelled` and whether the rest is still
    /// open.
    fn fetch_more(&mut self, fetch: Fetch, emit: &mut (dyn FnMut(QueryEvent) + Send))
    -> impl Future<Output = ()> + Send;

    /// Releases the open result without reading the rest. A no-op without
    /// one; never begins or ends a transaction the user opened.
    fn close_result(&mut self) -> impl Future<Output = ()> + Send;

    /// Namespaces below the connection (schemas, MySQL databases, SQLite
    /// attached databases), system ones included and flagged.
    fn schemas(&mut self) -> impl Future<Output = Result<Vec<SchemaInfo>>> + Send;

    /// Tables and views of one schema with their columns and foreign keys.
    fn introspect(&mut self, schema: &str) -> impl Future<Output = Result<SchemaModel>> + Send;

    /// Applies data editor changes to `table` as one unit, in order, with
    /// parameterized statements: in its own transaction, or, when the user
    /// has a transaction open, in a savepoint inside it that is released
    /// without committing. An update or delete whose key matches no row
    /// fails with [`ROW_NOT_FOUND`](crate::ROW_NOT_FOUND). Any failure undoes
    /// every change of the batch (and only those) and is reported as
    /// [`ApplyOutcome::Failed`]; `Err` is left for when the session itself is
    /// unusable.
    fn apply(
        &mut self,
        table: &TableRef,
        changes: &[RowChange],
    ) -> impl Future<Output = Result<ApplyOutcome>> + Send;

    /// Validates one statement the way the engine does before running it
    /// (syntax, and the names and types it resolves when preparing) without
    /// executing anything. `schema` is where unqualified names resolve for
    /// this check only, e.g. a console's current schema; `None` keeps the
    /// session's own.
    ///
    /// `Ok(None)` also covers statements the engine cannot validate without
    /// running them. `Err` is left for when the session itself failed.
    fn check(
        &mut self,
        sql: &str,
        schema: Option<&str>,
    ) -> impl Future<Output = Result<Option<SqlProblem>>> + Send;

    /// Makes `schema` where unqualified names resolve for the statements that
    /// follow: the head of Postgres' `search_path`, MySQL's current database.
    /// A no-op for SQLite, which resolves names across attached databases.
    fn set_schema(&mut self, schema: &str) -> impl Future<Output = Result<()>> + Send;
}

pub trait Canceller: Clone + Send + Sync + 'static {
    /// Requests cancellation of the running statement. A no-op when idle.
    fn cancel(&self) -> impl Future<Output = Result<()>> + Send;
}
