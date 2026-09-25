//! Schema introspection through `information_schema`, one query per catalog
//! kind for the whole schema rather than one per table.

use std::collections::HashMap;

use idedb_core::{ColumnInfo, ForeignKey, ObjectKind, SchemaInfo, SchemaModel, TableInfo};
use mysql_async::Conn;
use mysql_async::prelude::Queryable;

/// TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT.
type ColumnRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

const SYSTEM_SCHEMAS: [&str; 4] = ["information_schema", "mysql", "performance_schema", "sys"];

pub(crate) async fn schemas(conn: &mut Conn) -> mysql_async::Result<Vec<SchemaInfo>> {
    let names: Vec<String> = conn
        .query("select SCHEMA_NAME from information_schema.SCHEMATA order by SCHEMA_NAME")
        .await?;
    let mut schemas: Vec<SchemaInfo> = names
        .into_iter()
        .map(|name| {
            let is_system = SYSTEM_SCHEMAS.contains(&name.to_ascii_lowercase().as_str());
            SchemaInfo { name, is_system }
        })
        .collect();
    // User databases first, each group alphabetically (the sort is stable).
    schemas.sort_by_key(|s| s.is_system);
    Ok(schemas)
}

pub(crate) async fn introspect(conn: &mut Conn, schema: &str) -> mysql_async::Result<SchemaModel> {
    let tables: Vec<(String, String, Option<String>)> = conn
        .exec(
            "select TABLE_NAME, TABLE_TYPE, TABLE_COMMENT
             from information_schema.TABLES
             where TABLE_SCHEMA = ?
             order by TABLE_NAME",
            (schema,),
        )
        .await?;

    let mut model = SchemaModel::default();
    let mut index = HashMap::new();
    for (name, table_type, comment) in tables {
        let kind = match table_type.as_str() {
            "VIEW" | "SYSTEM VIEW" => ObjectKind::View,
            _ => ObjectKind::Table,
        };
        // MySQL reports the literal comment "VIEW" for every view.
        let comment =
            comment.filter(|c| !c.is_empty() && !(kind == ObjectKind::View && c == "VIEW"));
        index.insert(name.clone(), model.tables.len());
        model.tables.push(TableInfo {
            name,
            kind,
            comment,
            columns: Vec::new(),
            foreign_keys: Vec::new(),
        });
    }

    let columns: Vec<ColumnRow> = conn
        .exec(
            "select TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT, EXTRA
             from information_schema.COLUMNS
             where TABLE_SCHEMA = ?
             order by TABLE_NAME, ORDINAL_POSITION",
            (schema,),
        )
        .await?;
    for (table, name, type_name, nullable, default, comment, extra) in columns {
        let Some(&i) = index.get(&table) else {
            continue;
        };
        // EXTRA says `auto_increment`, `VIRTUAL GENERATED` or `STORED
        // GENERATED`; `DEFAULT_GENERATED` is an ordinary expression default.
        let extra = extra.to_ascii_lowercase();
        let generated = extra.contains("auto_increment")
            || extra.contains("virtual generated")
            || extra.contains("stored generated");
        model.tables[i].columns.push(ColumnInfo {
            name,
            type_name,
            nullable: nullable == "YES",
            default,
            primary_key: None,
            comment: comment.filter(|c| !c.is_empty()),
            generated,
        });
    }

    let primary_keys: Vec<(String, String, u32)> = conn
        .exec(
            "select TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION
             from information_schema.KEY_COLUMN_USAGE
             where TABLE_SCHEMA = ? and CONSTRAINT_NAME = 'PRIMARY'",
            (schema,),
        )
        .await?;
    for (table, column, position) in primary_keys {
        let Some(&i) = index.get(&table) else {
            continue;
        };
        if let Some(c) = model.tables[i]
            .columns
            .iter_mut()
            .find(|c| c.name == column)
        {
            c.primary_key = u16::try_from(position).ok();
        }
    }

    let foreign_keys: Vec<(String, String, String, String, String, String)> = conn
        .exec(
            "select TABLE_NAME, CONSTRAINT_NAME, COLUMN_NAME,
                    REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME
             from information_schema.KEY_COLUMN_USAGE
             where TABLE_SCHEMA = ? and REFERENCED_TABLE_NAME is not null
             order by TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
            (schema,),
        )
        .await?;
    for (table, name, column, referenced_schema, referenced_table, referenced_column) in
        foreign_keys
    {
        let Some(&i) = index.get(&table) else {
            continue;
        };
        let keys = &mut model.tables[i].foreign_keys;
        // Rows arrive grouped by constraint, in column order.
        match keys.last_mut() {
            Some(fk) if fk.name == name => {
                fk.columns.push(column);
                fk.referenced_columns.push(referenced_column);
            }
            _ => keys.push(ForeignKey {
                name,
                columns: vec![column],
                referenced_schema,
                referenced_table,
                referenced_columns: vec![referenced_column],
            }),
        }
    }

    Ok(model)
}
