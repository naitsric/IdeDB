//! Database object model produced by introspection.
//!
//! "Schema" is the engine's namespace level below the connection: a schema in
//! Postgres, a database in MySQL, an attached database in SQLite.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    /// Engine catalogs such as `pg_catalog` or `information_schema`.
    pub is_system: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

/// Everything in one schema, introspected in a single pass so the explorer,
/// completion and navigation all read from the same snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaModel {
    /// Sorted by name.
    pub tables: Vec<TableInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub name: String,
    pub kind: ObjectKind,
    pub comment: Option<String>,
    /// In ordinal order.
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// Declared type as the user would write it, e.g. `varchar(255)`, `numeric(10,2)`.
    pub type_name: String,
    pub nullable: bool,
    /// Default expression as SQL text.
    pub default: Option<String>,
    /// 1-based position within the primary key, if part of it.
    pub primary_key: Option<u16>,
    /// The database fills the value itself: auto-increment and identity
    /// columns, sequence defaults, computed (generated) columns, SQLite's
    /// rowid alias. A copied row leaves it for the database to fill.
    pub generated: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    /// Local columns, paired by position with `referenced_columns`.
    pub columns: Vec<String>,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}
