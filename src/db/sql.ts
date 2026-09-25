import type { Engine, Value } from "./api";

/** Quotes an identifier only when the engine would otherwise misread it. */
export function quoteIdent(engine: Engine, name: string): string {
  const plain = engine === "mysql" ? /^[A-Za-z_][A-Za-z0-9_$]*$/ : /^[a-z_][a-z0-9_$]*$/;
  if (plain.test(name) && !RESERVED.has(name.toLowerCase())) return name;
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

/** Common reserved words that must be quoted as identifiers in all three engines. */
const RESERVED = new Set([
  "all", "and", "as", "asc", "between", "by", "case", "check", "column", "constraint", "create",
  "cross", "default", "delete", "desc", "distinct", "drop", "else", "end", "exists", "false", "for",
  "foreign", "from", "full", "group", "having", "in", "index", "inner", "insert", "into", "is", "join",
  "key", "left", "like", "limit", "not", "null", "offset", "on", "or", "order", "outer", "primary",
  "references", "right", "select", "set", "table", "then", "to", "true", "union", "unique", "update",
  "user", "using", "values", "when", "where", "with",
]);
