//! Catalog introspection. One query per catalog kind for a whole schema, so
//! cost does not grow with round trips per table.

use std::collections::HashMap;

use idedb_core::{ColumnInfo, ForeignKey, ObjectKind, SchemaInfo, SchemaModel, TableInfo};
use tokio_postgres::Client;

pub(crate) async fn schemas(client: &Client) -> Result<Vec<SchemaInfo>, tokio_postgres::Error> {
    let rows = client
        .query(
            r"select nspname::text,
                     nspname in ('pg_catalog', 'information_schema') or nspname like 'pg\_%'
              from pg_namespace
              where nspname not like 'pg\_toast%' and nspname not like 'pg\_temp\_%'
              order by 2, 1",
            &[],
        )
        .await?;
    Ok(rows.iter().map(|r| SchemaInfo { name: r.get(0), is_system: r.get(1) }).collect())
}

pub(crate) async fn schema(client: &Client, schema: &str) -> Result<SchemaModel, tokio_postgres::Error> {
    // Partitions are left out: they show up through their parent table.
    let columns = client
        .query(
            "select c.relname::text, c.relkind::text, obj_description(c.oid, 'pg_class'),
                    a.attname::text, format_type(a.atttypid, a.atttypmod), not a.attnotnull,
                    pg_get_expr(d.adbin, d.adrelid), col_description(c.oid, a.attnum)
             from pg_class c
             join pg_namespace n on n.oid = c.relnamespace
             left join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
             left join pg_attrdef d on d.adrelid = c.oid and d.adnum = a.attnum
             where n.nspname = $1 and c.relkind in ('r', 'p', 'v', 'm', 'f') and not c.relispartition
             order by c.relname, a.attnum",
            &[&schema],
        )
        .await?;

    let primary_keys = client
        .query(
            "select c.relname::text, a.attname::text, array_position(con.conkey, a.attnum)
             from pg_constraint con
             join pg_class c on c.oid = con.conrelid
             join pg_namespace n on n.oid = c.relnamespace
             join pg_attribute a on a.attrelid = c.oid and a.attnum = any(con.conkey)
             where n.nspname = $1 and con.contype = 'p'",
            &[&schema],
        )
        .await?;

    let foreign_keys = client
        .query(
            "select c.relname::text, con.conname::text, rn.nspname::text, rc.relname::text,
                    array(select a.attname::text
                          from unnest(con.conkey) with ordinality k(num, ord)
                          join pg_attribute a on a.attrelid = con.conrelid and a.attnum = k.num
                          order by k.ord),
                    array(select a.attname::text
                          from unnest(con.confkey) with ordinality k(num, ord)
                          join pg_attribute a on a.attrelid = con.confrelid and a.attnum = k.num
                          order by k.ord)
             from pg_constraint con
             join pg_class c on c.oid = con.conrelid
             join pg_namespace n on n.oid = c.relnamespace
             join pg_class rc on rc.oid = con.confrelid
             join pg_namespace rn on rn.oid = rc.relnamespace
             where n.nspname = $1 and con.contype = 'f'
             order by c.relname, con.conname",
            &[&schema],
        )
        .await?;

    let pk_position: HashMap<(String, String), u16> = primary_keys
        .iter()
        .map(|r| ((r.get(0), r.get(1)), r.get::<_, i32>(2) as u16))
        .collect();

    let mut tables: Vec<TableInfo> = Vec::new();
    for row in &columns {
        let table: String = row.get(0);
        if tables.last().is_none_or(|t| t.name != table) {
            tables.push(TableInfo {
                name: table.clone(),
                kind: match row.get::<_, String>(1).as_str() {
                    "v" => ObjectKind::View,
                    "m" => ObjectKind::MaterializedView,
                    "f" => ObjectKind::ForeignTable,
                    _ => ObjectKind::Table,
                },
                comment: row.get(2),
                columns: Vec::new(),
                foreign_keys: Vec::new(),
            });
        }
        // Tables without columns come through the left join as one null row.
        let Some(name) = row.get::<_, Option<String>>(3) else { continue };
        let primary_key = pk_position.get(&(table, name.clone())).copied();
        tables.last_mut().unwrap().columns.push(ColumnInfo {
            name,
            type_name: row.get(4),
            nullable: row.get(5),
            default: row.get(6),
            primary_key,
            comment: row.get(7),
        });
    }

    let index: HashMap<String, usize> =
        tables.iter().enumerate().map(|(i, t)| (t.name.clone(), i)).collect();
    for row in &foreign_keys {
        let Some(&i) = index.get(&row.get::<_, String>(0)) else { continue };
        tables[i].foreign_keys.push(ForeignKey {
            name: row.get(1),
            columns: row.get(4),
            referenced_schema: row.get(2),
            referenced_table: row.get(3),
            referenced_columns: row.get(5),
        });
    }

    Ok(SchemaModel { tables })
}
