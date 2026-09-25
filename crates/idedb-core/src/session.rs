use std::future::Future;

use crate::{ApplyOutcome, QueryEvent, Result, RowChange, SchemaInfo, SchemaModel, ServerInfo, TableRef};

/// An open connection to one database server, implemented by each driver.
///
/// A session runs one statement at a time; callers serialize access. The app
/// dispatches over the concrete drivers with an enum, so the trait uses
/// `async fn`-style methods rather than being object safe.
pub trait Session: Send + 'static {
    type Canceller: Canceller;

    /// Handle that cancels whatever this session is running, usable while the
    /// session itself is busy.
    fn canceller(&self) -> Self::Canceller;

    /// Captured when the session connected.
    fn server_info(&self) -> &ServerInfo;

    /// Runs one statement, reporting through `emit`, in order:
    /// `Columns` (only for statements that return rows), `Rows` in pages of
    /// at most `page_size` rows, then exactly one `Done` or `Error`.
    ///
    /// Never fails itself: every outcome, including a cancellation
    /// (`Done { cancelled: true }`), is an event. The session stays usable
    /// afterwards.
    fn execute(
        &mut self,
        sql: &str,
        page_size: usize,
        emit: &mut (dyn FnMut(QueryEvent) + Send),
    ) -> impl Future<Output = ()> + Send;

    /// Namespaces below the connection (schemas, MySQL databases, SQLite
    /// attached databases), system ones included and flagged.
    fn schemas(&mut self) -> impl Future<Output = Result<Vec<SchemaInfo>>> + Send;

    /// Tables and views of one schema with their columns and foreign keys.
    fn introspect(&mut self, schema: &str) -> impl Future<Output = Result<SchemaModel>> + Send;

    /// Applies data editor changes to `table` in one transaction, in order,
    /// with parameterized statements. An update or delete whose key matches
    /// no row fails with [`ROW_NOT_FOUND`](crate::ROW_NOT_FOUND). Any failure
    /// rolls everything back and is reported as [`ApplyOutcome::Failed`];
    /// `Err` is left for when the session itself is unusable.
    fn apply(
        &mut self,
        table: &TableRef,
        changes: &[RowChange],
    ) -> impl Future<Output = Result<ApplyOutcome>> + Send;
}

pub trait Canceller: Clone + Send + Sync + 'static {
    /// Requests cancellation of the running statement. A no-op when idle.
    fn cancel(&self) -> impl Future<Output = Result<()>> + Send;
}
