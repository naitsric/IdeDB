import { Channel, invoke } from "@tauri-apps/api/core";
import { decode } from "@msgpack/msgpack";

/* Mirrors of the Rust types in crates/idedb-core, crates/idedb-store and src-tauri. */

export type Engine = "postgres" | "mysql" | "sqlite";
export type SslMode = "disable" | "prefer" | "require" | "verify-full";

export interface ConnectionParams {
  engine: Engine;
  host: string;
  port: number | null;
  user: string;
  database: string;
  sslMode: SslMode;
  path: string;
}

export interface DataSource {
  /** Empty until saved. */
  id: string;
  name: string;
  params: ConnectionParams;
  color: string | null;
  savePassword: boolean;
}

export interface ServerInfo {
  engine: Engine;
  version: string;
  defaultSchema: string | null;
}

export interface SchemaInfo {
  name: string;
  isSystem: boolean;
}

export type ObjectKind = "table" | "view" | "materializedView" | "foreignTable";

export interface ColumnInfo {
  name: string;
  typeName: string;
  nullable: boolean;
  default: string | null;
  primaryKey: number | null;
  comment: string | null;
}

export interface ForeignKey {
  name: string;
  columns: string[];
  referencedSchema: string;
  referencedTable: string;
  referencedColumns: string[];
}

export interface TableInfo {
  name: string;
  kind: ObjectKind;
  comment: string | null;
  columns: ColumnInfo[];
  foreignKeys: ForeignKey[];
}

export interface SchemaModel {
  tables: TableInfo[];
}

/** int8 values beyond ±2^32 arrive as bigint so they never lose precision. */
export type Value = null | boolean | number | bigint | string | Uint8Array;

export interface Column {
  name: string;
  typeName: string;
}

export type QueryEvent =
  | { kind: "columns"; columns: Column[] }
  | { kind: "rows"; rows: Value[][] }
  | { kind: "done"; rowCount: number; elapsedMs: number; cancelled: boolean }
  /** `position`: code points into the statement where the engine located the error. */
  | { kind: "error"; message: string; position: number | null };

export type Row = Value[];

/* Data editor changes; mirrors crates/idedb-core/src/edit.rs. */

export interface TableRef {
  schema: string;
  name: string;
}

export interface ColumnValue {
  column: string;
  value: Value;
}

export type RowChange =
  | { kind: "insert"; values: ColumnValue[] }
  | { kind: "update"; key: ColumnValue[]; values: ColumnValue[] }
  | { kind: "delete"; key: ColumnValue[] };

export type ApplyOutcome =
  /** `rows` pairs with the changes: the stored row, or null (deletes, unreadable inserts). */
  | { status: "applied"; rows: (Row | null)[] }
  /** Change `index` failed; nothing was written. */
  | { status: "failed"; index: number; message: string };

/** JSON cannot carry bigint or bytes: bigint goes as text (every engine casts it back), bytes as a number array. */
function toJsonValue(value: Value): unknown {
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Uint8Array) return Array.from(value);
  return value;
}

function toJsonChange(change: RowChange): unknown {
  const values = (list: ColumnValue[]) => list.map((c) => ({ column: c.column, value: toJsonValue(c.value) }));
  switch (change.kind) {
    case "insert":
      return { kind: "insert", values: values(change.values) };
    case "update":
      return { kind: "update", key: values(change.key), values: values(change.values) };
    case "delete":
      return { kind: "delete", key: values(change.key) };
  }
}

export interface HistoryEntry {
  id: number;
  dataSourceId: string;
  sql: string;
  /** UTC, RFC 3339. */
  executedAt: string;
  elapsedMs: number | null;
  rowCount: number | null;
  error: string | null;
}

export type ErrorCode = "passwordRequired" | "notFound" | "connect" | "query" | "invalidParams" | "storage";

export interface CommandError {
  code: ErrorCode;
  message: string;
}

export function isCommandError(e: unknown, code?: ErrorCode): e is CommandError {
  const candidate = e as CommandError | null;
  return typeof candidate?.message === "string" && (code === undefined || candidate.code === code);
}

export function errorMessage(e: unknown): string {
  return isCommandError(e) ? e.message : String(e);
}

export type SessionId = number;

export const api = {
  listDataSources: () => invoke<DataSource[]>("data_sources_list"),

  /** `password`: `undefined` keeps the stored one. */
  saveDataSource: (source: DataSource, password?: string) =>
    invoke<DataSource>("data_source_save", { source, password: password ?? null }),

  deleteDataSource: (id: string) => invoke<void>("data_source_delete", { id }),

  testDataSource: (source: DataSource, password?: string) =>
    invoke<{ server: ServerInfo; latencyMs: number }>("data_source_test", { source, password: password ?? null }),

  openSession: (dataSourceId: string, password?: string) =>
    invoke<{ id: SessionId; server: ServerInfo }>("session_open", { dataSourceId, password: password ?? null }),

  closeSession: (id: SessionId) => invoke<void>("session_close", { id }),

  cancel: (id: SessionId) => invoke<void>("session_cancel", { id }),

  schemas: (id: SessionId) => invoke<SchemaInfo[]>("session_schemas", { id }),

  introspect: (id: SessionId, schema: string) => invoke<SchemaModel>("session_introspect", { id, schema }),

  /** Newest first; `search` is a case-insensitive substring. */
  /** Applies data editor changes in one transaction; the outcome comes back as MessagePack. */
  async apply(id: SessionId, table: TableRef, changes: RowChange[]): Promise<ApplyOutcome> {
    const bytes = await invoke<ArrayBuffer>("session_apply", { id, table, changes: changes.map(toJsonChange) });
    return decode(new Uint8Array(bytes), { useBigInt64: true }) as ApplyOutcome;
  },

  /** Writes one chunk of an export; `first` truncates the file. */
  exportWrite: (path: string, chunk: string, first: boolean) => invoke<void>("export_write", { path, chunk, first }),

  history: (dataSourceId: string | null, search: string | null, limit: number) =>
    invoke<HistoryEntry[]>("history_list", { dataSourceId, search, limit }),

  /**
   * Runs one statement. Events arrive in order as MessagePack over a Tauri
   * channel; the promise resolves after the final `done` or `error`.
   */
  execute(id: SessionId, sql: string, onEvent: (event: QueryEvent) => void): Promise<void> {
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buffer) => {
      const event = decode(new Uint8Array(buffer), { useBigInt64: true }) as QueryEvent;
      if (event.kind === "done") event.rowCount = Number(event.rowCount);
      onEvent(event);
    };
    return invoke("session_execute", { id, sql, onEvent: channel });
  },
};
