//! Engine-agnostic types shared by the drivers and the app.
//!
//! Drivers implement [`Session`]; the app and the UI only ever see the types
//! defined here. Result rows stream to the UI as MessagePack-encoded
//! [`QueryEvent`]s, so the UI decodes one format regardless of the engine.

mod connection;
mod edit;
mod fetch;
mod schema;
mod session;
#[cfg(feature = "testing")]
pub mod testing;

pub use connection::{ConnectOptions, ConnectionParams, Engine, ServerInfo, SslMode};
pub use edit::{ApplyOutcome, ColumnValue, ROW_NOT_FOUND, RowChange, TableRef};
pub use fetch::{Fetch, Paged, Pager};
pub use schema::{ColumnInfo, ForeignKey, ObjectKind, SchemaInfo, SchemaModel, TableInfo};
pub use session::{Canceller, Session};

use serde::{Deserialize, Serialize};

/// A single cell value.
///
/// Serialized untagged so MessagePack carries native types (nil, bool, int,
/// float, str, bin) and the UI needs no per-cell envelope. Values without a
/// lossless native representation (numeric, timestamps, uuid, json) travel as
/// their canonical text form; the column's `type_name` tells the UI how to
/// render and edit them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(#[serde(with = "serde_bytes")] Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// Engine type name as the user would write it (`int8`, `varchar`, ...).
    pub type_name: String,
}

pub type Row = Vec<Value>;

/// Messages streamed to the UI while a statement runs, in this order:
/// `Columns`, zero or more `Rows`, then exactly one of `Done` or `Error`.
/// Continuing an open result ([`Session::fetch_more`]) sends `Rows` and the
/// final event only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueryEvent {
    Columns {
        columns: Vec<Column>,
    },
    Rows {
        rows: Vec<Row>,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        /// Rows returned by this call, or rows affected for DML.
        row_count: u64,
        elapsed_ms: u64,
        cancelled: bool,
        /// Reading paused at the fetch limit and the rest of the result is
        /// still open on the session, for `fetch_more`.
        has_more: bool,
        /// Whether the session is inside a transaction the user opened
        /// (`BEGIN`, or autocommit off), as of after this statement.
        in_transaction: bool,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        message: String,
        /// Where the engine located the error, as a 0-based offset in Unicode
        /// scalar values (Rust `char`s) into the executed statement text.
        position: Option<u32>,
        /// Same as in `Done`.
        in_transaction: bool,
    },
}

/// A problem the engine found in a statement it was asked to check (see
/// [`Session::check`]) without running it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlProblem {
    pub message: String,
    /// Same convention as [`QueryEvent::Error`]'s `position`.
    pub position: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("{0}")]
    Query(String),
    #[error("invalid connection settings: {0}")]
    InvalidParams(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The `Error` message of `fetch_more` when the session has no open result:
/// it was read to the end, closed, or replaced by something else the
/// session did (another statement, a data editor submit, an idle timeout).
pub const NO_OPEN_RESULT: &str = "The result is no longer open: run the statement again to fetch more rows.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_event_roundtrips_through_msgpack() {
        let event = QueryEvent::Rows {
            rows: vec![vec![
                Value::Null,
                Value::Bool(true),
                Value::Int(i64::MAX),
                Value::Float(1.5),
                Value::Text("héllo".into()),
                Value::Bytes(vec![0, 255]),
            ]],
        };
        let bytes = rmp_serde::to_vec_named(&event).unwrap();
        let decoded: QueryEvent = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, event);
    }
}
