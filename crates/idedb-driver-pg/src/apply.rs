//! Data editor changes: parameterized DML inside one transaction.
//!
//! Every value is bound as text and cast in SQL to its column's declared
//! type (`$1::text::numeric(12,2)`), so any type Postgres can parse from
//! text works without a Rust-side conversion per type. Updates and inserts
//! use `RETURNING *` so the editor shows what was actually stored.

use std::collections::HashMap;
use std::fmt::Write as _;

use idedb_core::{ApplyOutcome, ColumnValue, ROW_NOT_FOUND, Row, RowChange, TableRef, Value};
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{Client, Transaction};

use crate::decode::Cell;
use crate::{format_error, quote};

pub(crate) async fn apply(
    client: &mut Client,
    table: &TableRef,
    changes: &[RowChange],
) -> Result<ApplyOutcome, tokio_postgres::Error> {
    let types = column_types(client, table).await?;
    let target = format!("{}.{}", quote(&table.schema), quote(&table.name));

    let tx = client.transaction().await?;
    let mut rows = Vec::with_capacity(changes.len());
    for (index, change) in changes.iter().enumerate() {
        match apply_one(&tx, &target, &types, change).await {
            Ok(row) => rows.push(row),
            Err(message) => {
                // Dropping `tx` would roll back too, but asynchronously.
                let _ = tx.rollback().await;
                return Ok(ApplyOutcome::Failed { index, message });
            }
        }
    }
    tx.commit().await?;
    Ok(ApplyOutcome::Applied { rows })
}

/// Declared type of each column, as `format_type` prints it.
async fn column_types(client: &Client, table: &TableRef) -> Result<HashMap<String, String>, tokio_postgres::Error> {
    let rows = client
        .query(
            "select a.attname::text, format_type(a.atttypid, a.atttypmod)
             from pg_attribute a
             join pg_class c on c.oid = a.attrelid
             join pg_namespace n on n.oid = c.relnamespace
             where n.nspname = $1 and c.relname = $2 and a.attnum > 0 and not a.attisdropped",
            &[&table.schema, &table.name],
        )
        .await?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

async fn apply_one(
    tx: &Transaction<'_>,
    target: &str,
    types: &HashMap<String, String>,
    change: &RowChange,
) -> Result<Option<Row>, String> {
    let mut params: Vec<Option<String>> = Vec::new();
    let mut bind = |cv: &ColumnValue| -> Result<String, String> {
        let ty = types.get(&cv.column).ok_or_else(|| format!("unknown column {}", cv.column))?;
        params.push(text_param(&cv.value));
        Ok(format!("${}::text::{ty}", params.len()))
    };

    let (sql, returns_row) = match change {
        RowChange::Insert { values } if values.is_empty() => (format!("insert into {target} default values returning *"), true),
        RowChange::Insert { values } => {
            let columns = values.iter().map(|v| quote(&v.column)).collect::<Vec<_>>().join(", ");
            let placeholders = values.iter().map(&mut bind).collect::<Result<Vec<_>, _>>()?.join(", ");
            (format!("insert into {target} ({columns}) values ({placeholders}) returning *"), true)
        }
        RowChange::Update { key, values } => {
            let mut sql = format!("update {target} set ");
            for (i, v) in values.iter().enumerate() {
                let placeholder = bind(v)?;
                let _ = write!(sql, "{}{} = {placeholder}", if i > 0 { ", " } else { "" }, quote(&v.column));
            }
            sql.push_str(&where_clause(key, &mut bind)?);
            sql.push_str(" returning *");
            (sql, true)
        }
        RowChange::Delete { key } => (format!("delete from {target}{}", where_clause(key, &mut bind)?), false),
    };

    let statement = tx
        .prepare_typed(&sql, &vec![Type::TEXT; params.len()])
        .await
        .map_err(|e| format_error(&e))?;
    let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();

    if !returns_row {
        let affected = tx.execute(&statement, &refs).await.map_err(|e| format_error(&e))?;
        return if affected == 0 { Err(ROW_NOT_FOUND.into()) } else { Ok(None) };
    }
    let rows = tx.query(&statement, &refs).await.map_err(|e| format_error(&e))?;
    let row = rows.first().ok_or_else(|| ROW_NOT_FOUND.to_owned())?;
    Ok(Some((0..row.len()).map(|i| row.get::<_, Cell>(i).0).collect()))
}

fn where_clause(
    key: &[ColumnValue],
    bind: &mut impl FnMut(&ColumnValue) -> Result<String, String>,
) -> Result<String, String> {
    if key.is_empty() {
        return Err("a row change needs the row's primary key".into());
    }
    let mut out = String::from(" where ");
    for (i, k) in key.iter().enumerate() {
        let placeholder = bind(k)?;
        let _ = write!(out, "{}{} = {placeholder}", if i > 0 { " and " } else { "" }, quote(&k.column));
    }
    Ok(out)
}

/// A value in the text form Postgres parses for any type.
fn text_param(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        // bytea's hex input format.
        Value::Bytes(bytes) => {
            let mut out = String::with_capacity(2 + bytes.len() * 2);
            out.push_str("\\x");
            for b in bytes {
                let _ = write!(out, "{b:02x}");
            }
            out
        }
    })
}
