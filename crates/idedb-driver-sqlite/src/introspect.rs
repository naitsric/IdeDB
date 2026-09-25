//! Schema introspection from `sqlite_schema` and the table-valued pragmas.
//!
//! In SQLite a "schema" is an attached database: `main`, `temp` or any
//! `ATTACH`ed file. Column facts are reported as the pragmas state them; in
//! particular `nullable` is `not notnull`, so an `INTEGER PRIMARY KEY`
//! (rowid alias) without `NOT NULL` shows as nullable even though SQLite
//! never stores NULL in it.

use idedb_core::{ColumnInfo, ForeignKey, ObjectKind, SchemaInfo, SchemaModel, TableInfo};
use rusqlite::Connection;

use crate::quote;

pub(crate) fn schemas(conn: &Connection) -> rusqlite::Result<Vec<SchemaInfo>> {
    let mut statement = conn.prepare("select name from pragma_database_list order by seq")?;
    let names = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

    let mut schemas = Vec::with_capacity(names.len());
    for name in names {
        // `temp` always exists; only list it once something lives there.
        if name == "temp" {
            let used: bool = conn.query_row("select exists (select 1 from temp.sqlite_schema)", [], |r| r.get(0))?;
            if !used {
                continue;
            }
        }
        schemas.push(SchemaInfo { name, is_system: false });
    }
    Ok(schemas)
}

pub(crate) fn schema_model(conn: &Connection, schema: &str) -> rusqlite::Result<SchemaModel> {
    let objects: Vec<(String, ObjectKind)> = conn
        .prepare(&format!(
            "select name, type from {}.sqlite_schema
             where type in ('table', 'view') and name not like 'sqlite\\_%' escape '\\'
             order by name",
            quote(schema)
        ))?
        .query_map([], |r| {
            let kind = if r.get::<_, String>(1)? == "view" { ObjectKind::View } else { ObjectKind::Table };
            Ok((r.get(0)?, kind))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut tables = Vec::with_capacity(objects.len());
    for (name, kind) in objects {
        let columns = columns(conn, schema, &name)?;
        let foreign_keys = if kind == ObjectKind::Table { foreign_keys(conn, schema, &name)? } else { Vec::new() };
        tables.push(TableInfo { name, kind, comment: None, columns, foreign_keys });
    }
    Ok(SchemaModel { tables })
}

fn columns(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<ColumnInfo>> {
    // hidden = 1 marks virtual-table hidden columns; generated columns (2, 3) are real columns.
    conn.prepare(
        r#"select name, type, "notnull", dflt_value, pk from pragma_table_xinfo(?1, ?2)
           where hidden <> 1 order by cid"#,
    )?
    .query_map([table, schema], |r| {
        let pk: u16 = r.get(4)?;
        Ok(ColumnInfo {
            name: r.get(0)?,
            type_name: r.get::<_, String>(1)?.to_lowercase(),
            nullable: !r.get::<_, bool>(2)?,
            default: r.get(3)?,
            primary_key: (pk > 0).then_some(pk),
            comment: None,
        })
    })?
    .collect()
}

fn foreign_keys(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<ForeignKey>> {
    // One row per column pair; `to` is NULL when the constraint references
    // the parent's primary key implicitly.
    let rows: Vec<(i64, String, String, Option<String>)> = conn
        .prepare(r#"select id, "table", "from", "to" from pragma_foreign_key_list(?1, ?2) order by id, seq"#)?
        .query_map([table, schema], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut keys: Vec<(i64, ForeignKey, bool)> = Vec::new();
    for (id, parent, from, to) in rows {
        if keys.last().is_none_or(|(last, ..)| *last != id) {
            keys.push((
                id,
                ForeignKey {
                    // SQLite does not expose constraint names through the pragmas.
                    name: format!("fk_{table}_{id}"),
                    columns: Vec::new(),
                    referenced_schema: schema.to_owned(),
                    referenced_table: parent,
                    referenced_columns: Vec::new(),
                },
                false,
            ));
        }
        let (_, key, implicit) = keys.last_mut().expect("pushed above");
        key.columns.push(from);
        match to {
            Some(to) => key.referenced_columns.push(to),
            None => *implicit = true,
        }
    }

    keys.into_iter()
        .map(|(_, mut key, implicit)| {
            if implicit {
                key.referenced_columns = primary_key(conn, schema, &key.referenced_table)?;
            }
            Ok(key)
        })
        .collect()
}

fn primary_key(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare("select name from pragma_table_info(?1, ?2) where pk > 0 order by pk")?
        .query_map([table, schema], |r| r.get(0))?
        .collect()
}
