import type { Engine, Value } from "./api";

/**
 * Quotes an identifier only when the engine would otherwise misread it: a
 * character outside plain identifiers (in Postgres, any uppercase letter)
 * or one of the engine's keywords. Quoting a name that did not need it
 * changes nothing, so the keyword lists err on the side of quoting.
 */
export function quoteIdent(engine: Engine, name: string): string {
  const plain = engine === "mysql" ? /^[A-Za-z_][A-Za-z0-9_$]*$/ : /^[a-z_][a-z0-9_$]*$/;
  if (plain.test(name) && !RESERVED[engine].has(name.toLowerCase())) return name;
  return engine === "mysql" ? `\`${name.replaceAll("`", "``")}\`` : `"${name.replaceAll('"', '""')}"`;
}

/** `schema.table`, leaving the schema out when it is the one unqualified names resolve to. */
export function qualifiedName(engine: Engine, schema: string, table: string, defaultSchema: string | null): string {
  const t = quoteIdent(engine, table);
  return schema === defaultSchema ? t : `${quoteIdent(engine, schema)}.${t}`;
}

export function selectAll(engine: Engine, schema: string, table: string, defaultSchema: string | null): string {
  return `select * from ${qualifiedName(engine, schema, table, defaultSchema)}`;
}

/** WHERE and ORDER BY of the data editor's filter bar, as SQL text without the keywords. */
export interface TableFilter {
  where?: string;
  orderBy?: string;
}

/** The query behind a table's data editor. */
export function tableQuery(
  engine: Engine,
  schema: string,
  table: string,
  defaultSchema: string | null,
  filter: TableFilter = {},
): string {
  let sql = selectAll(engine, schema, table, defaultSchema);
  if (filter.where?.trim()) sql += ` where ${filter.where.trim()}`;
  if (filter.orderBy?.trim()) sql += ` order by ${filter.orderBy.trim()}`;
  return sql;
}

/**
 * A value as an SQL literal, for text the user sees and edits (Copy as
 * INSERT, filter conditions). Strings are always quoted: every engine
 * coerces a quoted literal to the column's type. Never used to build
 * statements the app runs on its own; those are parameterized.
 */
export function sqlLiteral(engine: Engine, value: Value): string {
  if (value === null) return "NULL";
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  if (typeof value === "number") return Number.isFinite(value) ? String(value) : `'${value}'`;
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Uint8Array) {
    let hex = "";
    for (const b of value) hex += b.toString(16).padStart(2, "0");
    return engine === "postgres" ? `'\\x${hex}'::bytea` : `X'${hex}'`;
  }
  // MySQL treats backslash as an escape character in string literals.
  const escaped = engine === "mysql" ? value.replaceAll("\\", "\\\\") : value;
  return `'${escaped.replaceAll("'", "''")}'`;
}

const words = (list: string) => new Set(list.trim().split(/\s+/));

/**
 * Keywords that cannot stand as a bare identifier everywhere one can go.
 * Postgres: its reserved keywords, plus those that may only be function or
 * type names and those that may not be (appendix C of its manual).
 * MySQL: the 8.x reserved words. SQLite: all of its keywords, since
 * which ones it accepts as names depends on the position.
 */
const RESERVED: Record<Engine, ReadonlySet<string>> = {
  postgres: words(`
    all analyse analyze and any array as asc asymmetric authorization between bigint binary bit
    boolean both case cast char character check coalesce collate collation column concurrently
    constraint create cross current_catalog current_date current_role current_schema current_time
    current_timestamp current_user dec decimal default deferrable desc distinct do else end except
    exists extract false fetch float for foreign freeze from full grant greatest group grouping
    having ilike in initially inner inout int integer intersect interval into is isnull join
    json json_array json_arrayagg json_exists json_object json_objectagg json_query json_scalar
    json_serialize json_table json_value lateral leading least left like limit localtime
    localtimestamp merge_action national natural nchar none normalize not notnull null nullif
    numeric offset on only or order out outer overlaps overlay placing position precision primary
    real references returning right row select session_user setof similar smallint some substring
    symmetric system_user table tablesample then time timestamp to trailing treat trim true union
    unique user using values varchar variadic verbose when where window with xmlattributes
    xmlconcat xmlelement xmlexists xmlforest xmlnamespaces xmlparse xmlpi xmlroot xmlserialize
    xmltable
  `),
  mysql: words(`
    accessible add all alter analyze and as asc asensitive before between bigint binary blob both
    by call cascade case change char character check collate column condition constraint continue
    convert create cross cube cume_dist current_date current_time current_timestamp current_user
    cursor database databases day_hour day_microsecond day_minute day_second dec decimal declare
    default delayed delete dense_rank desc describe deterministic distinct distinctrow div double
    drop dual each else elseif empty enclosed escaped except exists exit explain false fetch
    first_value float float4 float8 for force foreign from fulltext function generated get grant
    group grouping groups having high_priority hour_microsecond hour_minute hour_second if ignore
    in index infile inner inout insensitive insert int int1 int2 int3 int4 int8 integer intersect
    interval into io_after_gtids io_before_gtids is iterate join json_table key keys kill lag
    last_value lateral lead leading leave left like limit linear lines load localtime
    localtimestamp lock long longblob longtext loop low_priority manual master_bind
    master_ssl_verify_server_cert match maxvalue mediumblob mediumint mediumtext middleint
    minute_microsecond minute_second mod modifies natural no_write_to_binlog not nth_value ntile
    null numeric of on optimize optimizer_costs option optionally or order out outer outfile over
    parallel partition percent_rank precision primary procedure purge qualify range rank read reads
    read_write real recursive references regexp release rename repeat replace require resignal
    restrict return revoke right rlike row row_number rows schema schemas second_microsecond select
    sensitive separator set show signal smallint spatial specific sql sql_big_result
    sql_calc_found_rows sql_small_result sqlexception sqlstate sqlwarning ssl starting stored
    straight_join system table terminated then tinyblob tinyint tinytext to trailing trigger true
    undo union unique unlock unsigned update usage use using utc_date utc_time utc_timestamp values
    varbinary varchar varcharacter varying virtual when where while window with write xor
    year_month zerofill
  `),
  sqlite: words(`
    abort action add after all alter always analyze and as asc attach autoincrement before begin
    between by cascade case cast check collate column commit conflict constraint create cross
    current current_date current_time current_timestamp database default deferrable deferred delete
    desc detach distinct do drop each else end escape except exclude exclusive exists explain fail
    filter first following for foreign from full generated glob group groups having if ignore
    immediate in index indexed initially inner insert instead intersect into is isnull join key
    last left like limit match materialized natural no not nothing notnull null nulls of offset on
    or order others outer over partition plan pragma preceding primary query raise range recursive
    references regexp reindex release rename replace restrict returning right rollback row rows
    savepoint select set table temp temporary then ties to transaction trigger unbounded union
    unique update using vacuum values view virtual when where window with without
  `),
};
