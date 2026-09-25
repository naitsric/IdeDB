//! Data editor changes: parameterized DML inside one transaction.
//!
//! MySQL has no `RETURNING`, so after an insert or update the row is read
//! back by its primary key: key values from the change, or the generated id
//! for an `AUTO_INCREMENT` key column the insert left out.

use std::fmt::Write as _;

use idedb_core::{ApplyOutcome, ColumnValue, ROW_NOT_FOUND, Row, RowChange, TableRef, Value};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Params, Transaction, TxOpts, Value as MyValue};

use crate::decode::{self, Kind};
use crate::format_error;

/// Primary key columns of a table, and which of them is `AUTO_INCREMENT`.
struct KeyInfo {
    columns: Vec<String>,
    auto_increment: Option<String>,
}

pub(crate) async fn apply(
    conn: &mut Conn,
    table: &TableRef,
    changes: &[RowChange],
) -> Result<ApplyOutcome, mysql_async::Error> {
    let keys = key_info(conn, table).await?;
    let target = format!("{}.{}", quote(&table.schema), quote(&table.name));

    let mut tx = conn.start_transaction(TxOpts::default()).await?;
    let mut rows = Vec::with_capacity(changes.len());
    for (index, change) in changes.iter().enumerate() {
        match apply_one(&mut tx, &target, &keys, change).await {
            Ok(row) => rows.push(row),
            Err(e) => {
                let message = e.message();
                tx.rollback().await?;
                return Ok(ApplyOutcome::Failed { index, message });
            }
        }
    }
    tx.commit().await?;
    Ok(ApplyOutcome::Applied { rows })
}

/// A failed change: a server error, or a rule of ours (row not found).
enum ChangeError {
    Server(mysql_async::Error),
    Rule(String),
}

impl ChangeError {
    fn message(self) -> String {
        match self {
            ChangeError::Server(e) => format_error(&e),
            ChangeError::Rule(message) => message,
        }
    }
}

impl From<mysql_async::Error> for ChangeError {
    fn from(e: mysql_async::Error) -> Self {
        ChangeError::Server(e)
    }
}

async fn key_info(conn: &mut Conn, table: &TableRef) -> Result<KeyInfo, mysql_async::Error> {
    let rows: Vec<(String, String)> = conn
        .exec(
            "select COLUMN_NAME, EXTRA from information_schema.COLUMNS
             where TABLE_SCHEMA = ? and TABLE_NAME = ? and COLUMN_KEY = 'PRI'
             order by ORDINAL_POSITION",
            (&table.schema, &table.name),
        )
        .await?;
    Ok(KeyInfo {
        auto_increment: rows
            .iter()
            .find(|(_, extra)| extra.to_lowercase().contains("auto_increment"))
            .map(|(name, _)| name.clone()),
        columns: rows.into_iter().map(|(name, _)| name).collect(),
    })
}

async fn apply_one(
    tx: &mut Transaction<'_>,
    target: &str,
    keys: &KeyInfo,
    change: &RowChange,
) -> Result<Option<Row>, ChangeError> {
    match change {
        RowChange::Insert { values } => {
            let columns = values.iter().map(|v| quote(&v.column)).collect::<Vec<_>>().join(", ");
            let placeholders = vec!["?"; values.len()].join(", ");
            tx.exec_drop(format!("insert into {target} ({columns}) values ({placeholders})"), params(values))
                .await?;

            // The stored row is found by the key values given, plus the
            // generated id for an AUTO_INCREMENT key left out.
            let generated = tx.last_insert_id();
            let mut key = Vec::with_capacity(keys.columns.len());
            for column in &keys.columns {
                if let Some(given) = values.iter().find(|v| &v.column == column) {
                    key.push(given.clone());
                } else if let (Some(id), true) = (generated, keys.auto_increment.as_ref() == Some(column)) {
                    key.push(ColumnValue { column: column.clone(), value: Value::Int(id as i64) });
                } else {
                    // A key filled by a default or trigger cannot be located.
                    return Ok(None);
                }
            }
            if key.is_empty() {
                return Ok(None);
            }
            read_back(tx, target, &key).await
        }
        RowChange::Update { key, values } => {
            let assignments = values
                .iter()
                .map(|v| format!("{} = ?", quote(&v.column)))
                .collect::<Vec<_>>()
                .join(", ");
            let mut bound = values.clone();
            bound.extend(key.iter().cloned());
            tx.exec_drop(format!("update {target} set {assignments}{}", where_clause(key)?), params(&bound))
                .await?;

            // Read back by the key as it is after the update: MySQL reports
            // changed rows, not matched ones, so a no-op update looks like a
            // miss and only the read tells them apart.
            let new_key: Vec<ColumnValue> = key
                .iter()
                .map(|k| values.iter().find(|v| v.column == k.column).unwrap_or(k).clone())
                .collect();
            match read_back(tx, target, &new_key).await? {
                Some(row) => Ok(Some(row)),
                None => Err(ChangeError::Rule(ROW_NOT_FOUND.into())),
            }
        }
        RowChange::Delete { key } => {
            tx.exec_drop(format!("delete from {target}{}", where_clause(key)?), params(key)).await?;
            if tx.affected_rows() == 0 {
                return Err(ChangeError::Rule(ROW_NOT_FOUND.into()));
            }
            Ok(None)
        }
    }
}

async fn read_back(tx: &mut Transaction<'_>, target: &str, key: &[ColumnValue]) -> Result<Option<Row>, ChangeError> {
    let row: Option<mysql_async::Row> =
        tx.exec_first(format!("select * from {target}{}", where_clause(key)?), params(key)).await?;
    Ok(row.map(|row| {
        let columns = row.columns();
        row.unwrap_raw()
            .into_iter()
            .zip(columns.iter())
            .map(|(value, column)| value.map_or(Value::Null, |v| decode::binary(column, Kind::of(column), v)))
            .collect()
    }))
}

fn where_clause(key: &[ColumnValue]) -> Result<String, ChangeError> {
    if key.is_empty() {
        return Err(ChangeError::Rule("a row change needs the row's primary key".into()));
    }
    let mut out = String::from(" where ");
    for (i, k) in key.iter().enumerate() {
        let _ = write!(out, "{}{} = ?", if i > 0 { " and " } else { "" }, quote(&k.column));
    }
    Ok(out)
}

fn params(values: &[ColumnValue]) -> Params {
    if values.is_empty() {
        return Params::Empty;
    }
    Params::Positional(
        values
            .iter()
            .map(|v| match &v.value {
                Value::Null => MyValue::NULL,
                Value::Bool(b) => MyValue::Int(i64::from(*b)),
                Value::Int(i) => MyValue::Int(*i),
                Value::Float(f) => MyValue::Double(*f),
                // MySQL converts strings to the column type on assignment and comparison.
                Value::Text(s) => MyValue::Bytes(s.as_bytes().to_vec()),
                Value::Bytes(b) => MyValue::Bytes(b.clone()),
            })
            .collect(),
    )
}

fn quote(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}
