//! Row changes made in the data editor, applied by [`Session::apply`].
//!
//! [`Session::apply`]: crate::Session::apply

use serde::{Deserialize, Serialize};

use crate::{Row, Value};

/// The table a set of changes writes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnValue {
    pub column: String,
    pub value: Value,
}

/// One row-level change. `key` holds the row's primary key values as they
/// were read, so the row is found even if the change edits key columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RowChange {
    /// Columns left out take their default.
    Insert { values: Vec<ColumnValue> },
    Update { key: Vec<ColumnValue>, values: Vec<ColumnValue> },
    Delete { key: Vec<ColumnValue> },
}

/// Result of applying a batch of changes as one unit: in the driver's own
/// transaction, or in a savepoint when the user has a transaction open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ApplyOutcome {
    /// Everything was written. `rows` pairs with the changes: the row as
    /// stored after an insert or update (defaults, generated ids and
    /// server-side conversions included, in table column order), `None` for
    /// deletes and for inserted rows the engine cannot read back.
    #[serde(rename_all = "camelCase")]
    Applied {
        rows: Vec<Option<Row>>,
        /// The changes went into the user's open transaction and are not
        /// committed: the user's COMMIT or ROLLBACK decides. `false` means
        /// they were committed.
        in_transaction: bool,
    },
    /// Change `index` failed and nothing of the batch was written. Inside a
    /// user transaction only the batch is undone; the transaction stays open.
    Failed { index: usize, message: String },
}

/// Message for an update or delete whose key matched no row.
pub const ROW_NOT_FOUND: &str = "The row no longer exists: it was deleted or its key changed since it was read";
