import type { Engine, TableRef, Value } from "../db/api";
import { qualifiedName, quoteIdent, sqlLiteral } from "../db/sql";

/**
 * Result data as text: clipboard (TSV, CSV, JSON, SQL INSERT) and file
 * export. Pure, so exports can be built chunk by chunk.
 */

export type ExportFormat = "tsv" | "csv" | "json" | "sql";

export const EXPORT_EXTENSION: Record<ExportFormat, string> = { tsv: "tsv", csv: "csv", json: "json", sql: "sql" };

/** How a value reads as plain text: NULL is empty, bytes are hex. */
function plainText(value: Value): string {
  if (value === null) return "";
  if (value instanceof Uint8Array) {
    let out = "0x";
    for (const b of value) out += b.toString(16).padStart(2, "0").toUpperCase();
    return out;
  }
  return String(value);
}

/** Quotes a field only when it contains the separator, a quote or a line break (RFC 4180 style). */
function delimited(value: Value, separator: string): string {
  const text = plainText(value);
  return text.includes(separator) || /["\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

export function toTsv(rows: readonly Value[][], header?: readonly string[]): string {
  const lines = rows.map((row) => row.map((v) => delimited(v, "\t")).join("\t"));
  if (header) lines.unshift(header.map((h) => delimited(h, "\t")).join("\t"));
  return lines.join("\n");
}

export function toCsv(rows: readonly Value[][], header?: readonly string[]): string {
  const lines = rows.map((row) => row.map((v) => delimited(v, ",")).join(","));
  if (header) lines.unshift(header.map((h) => delimited(h, ",")).join(","));
  return lines.join("\n");
}

/** JSON-safe value: bigint stays exact as a number only when it fits, bytes become hex. */
function jsonValue(value: Value): unknown {
  if (typeof value === "bigint") return Number.isSafeInteger(Number(value)) ? Number(value) : value.toString();
  if (value instanceof Uint8Array) return plainText(value);
  return value;
}

/** One JSON object per row, so large exports can be written in chunks. */
export function toJsonObjects(rows: readonly Value[][], columns: readonly string[]): string[] {
  return rows.map((row) => JSON.stringify(Object.fromEntries(columns.map((c, i) => [c, jsonValue(row[i])]))));
}

export function toJson(rows: readonly Value[][], columns: readonly string[]): string {
  const objects = toJsonObjects(rows, columns);
  return objects.length === 0 ? "[]" : `[\n  ${objects.join(",\n  ")}\n]`;
}

/** One `INSERT` per row; `table` null writes a placeholder name the user fills in. */
export function toInserts(
  engine: Engine,
  table: TableRef | null,
  defaultSchema: string | null,
  columns: readonly string[],
  rows: readonly Value[][],
): string {
  const target = table ? qualifiedName(engine, table.schema, table.name, defaultSchema) : "my_table";
  const columnList = columns.map((c) => quoteIdent(engine, c)).join(", ");
  return rows
    .map((row) => `insert into ${target} (${columnList}) values (${row.map((v) => sqlLiteral(engine, v)).join(", ")});`)
    .join("\n");
}
