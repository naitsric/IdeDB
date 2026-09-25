//! Data editor changes: parameterized DML inside one transaction, with
//! `RETURNING *` so the editor shows what was actually stored (type affinity
//! may convert what the user typed).

use std::fmt::Write as _;

use idedb_core::{ApplyOutcome, ColumnValue, ROW_NOT_FOUND, Row, RowChange, TableRef, Value};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params_from_iter};

use crate::{quote, value};

pub(crate) fn apply(conn: &mut Connection, table: &TableRef, changes: &[RowChange]) -> rusqlite::Result<ApplyOutcome> {
    let target = format!("{}.{}", quote(&table.schema), quote(&table.name));
    let tx = conn.transaction()?;
    let mut rows = Vec::with_capacity(changes.len());
    for (index, change) in changes.iter().enumerate() {
        match apply_one(&tx, &target, change) {
            Ok(row) => rows.push(row),
            Err(message) => {
                tx.rollback()?;
                return Ok(ApplyOutcome::Failed { index, message });
            }
        }
    }
    tx.commit()?;
    Ok(ApplyOutcome::Applied { rows })
}

fn apply_one(conn: &Connection, target: &str, change: &RowChange) -> Result<Option<Row>, String> {
    let (sql, bound): (String, Vec<&ColumnValue>) = match change {
        RowChange::Insert { values } if values.is_empty() => (format!("insert into {target} default values returning *"), vec![]),
        RowChange::Insert { values } => {
            let columns = values.iter().map(|v| quote(&v.column)).collect::<Vec<_>>().join(", ");
            let placeholders = vec!["?"; values.len()].join(", ");
            (format!("insert into {target} ({columns}) values ({placeholders}) returning *"), values.iter().collect())
        }
        RowChange::Update { key, values } => {
            let assignments = values
                .iter()
                .map(|v| format!("{} = ?", quote(&v.column)))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("update {target} set {assignments}{} returning *", where_clause(key)?);
            (sql, values.iter().chain(key).collect())
        }
        RowChange::Delete { key } => {
            let sql = format!("delete from {target}{}", where_clause(key)?);
            let changed = conn
                .execute(&sql, params_from_iter(key.iter().map(|k| param(&k.value))))
                .map_err(|e| e.to_string())?;
            return if changed == 0 { Err(ROW_NOT_FOUND.into()) } else { Ok(None) };
        }
    };

    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let width = statement.column_count();
    let mut rows = statement
        .query(params_from_iter(bound.iter().map(|v| param(&v.value))))
        .map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => Ok(Some((0..width).map(|i| value(row.get_ref_unwrap(i))).collect())),
        None => Err(ROW_NOT_FOUND.into()),
    }
}

fn where_clause(key: &[ColumnValue]) -> Result<String, String> {
    if key.is_empty() {
        return Err("a row change needs the row's primary key".into());
    }
    let mut out = String::from(" where ");
    for (i, k) in key.iter().enumerate() {
        let _ = write!(out, "{}{} = ?", if i > 0 { " and " } else { "" }, quote(&k.column));
    }
    Ok(out)
}

fn param(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Int(i) => SqlValue::Integer(*i),
        Value::Float(f) => SqlValue::Real(*f),
        Value::Text(s) => SqlValue::Text(s.clone()),
        Value::Bytes(b) => SqlValue::Blob(b.clone()),
    }
}
