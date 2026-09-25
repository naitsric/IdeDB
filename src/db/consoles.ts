import { create } from "zustand";
import { api, errorMessage, type Column, type SessionId, type TableRef, type Value } from "./api";
import { invalidateChecks } from "./checkRevisions";
import { openSessionFor, useDataSources } from "./dataSources";
import type { TableFilter } from "./sql";

/**
 * Query consoles. Each console is bound to one data source and opens its
 * own session lazily, on first execution, like a DataGrip console. Results
 * live in tabs: a run replaces the active tab unless it is pinned.
 * Console text persists across launches; results do not.
 */

export type ResultStatus = "running" | "done" | "cancelled" | "error";

export interface ResultMeta {
  id: number;
  title: string;
  pinned: boolean;
  sql: string;
  status: ResultStatus;
  columns: Column[];
  /** Rows received so far. Updated at most once per frame while streaming. */
  rowCount: number;
  /** Set for statements without a result set (DML, DDL). */
  affectedRows?: number;
  firstPageMs?: number;
  elapsedMs?: number;
  error?: string;
  /** Where the engine located the error: code points into `sql`. */
  errorPosition?: number;
  /** Set when the result is a table's data (the data editor), which makes it editable. */
  table?: TableRef;
  /** Bumped when loaded rows change in place (submitted edits), so the grid repaints. */
  version: number;
}

export interface ConsoleState {
  id: string;
  dataSourceId: string;
  /** Set for consoles opened to show a table's data, with the filter bar's state. */
  table?: TableRef & TableFilter;
  /**
   * Where unqualified names resolve in this console (schema selector), when
   * the user picked one other than the server default.
   */
  schema?: string;
  sql: string;
  sessionId?: SessionId;
  connecting?: boolean;
  connectError?: string;
  /** The session has a transaction the user opened, as of its last statement. */
  inTransaction?: boolean;
  results: ResultMeta[];
  activeResultId?: number;
  /** Results opened so far, for numbering new tabs. */
  resultCount: number;
}

interface ConsolesState {
  consoles: Record<string, ConsoleState>;
  create: (dataSourceId: string, sql?: string, table?: ConsoleState["table"]) => string;
  remove: (id: string) => Promise<void>;
  setSql: (id: string, sql: string) => void;
  /**
   * Runs one statement into a result tab and resolves to its final state, or
   * to `undefined` when nothing ran (busy console, no session).
   */
  runStatement: (
    id: string,
    sql: string,
    options?: { newTab?: boolean; table?: TableRef },
  ) => Promise<ResultMeta | undefined>;
  /** Changes a result's loaded rows in place and repaints its grid. */
  mutateRows: (id: string, resultId: number, change: (rows: Value[][]) => void) => void;
  /** Records whether the console's session has a user transaction open (after a data editor submit). */
  setInTransaction: (id: string, inTransaction: boolean) => void;
  setTableFilter: (id: string, filter: TableFilter) => void;
  /** Switches the console's current schema; `undefined` goes back to the server default. */
  setSchema: (id: string, schema: string | undefined) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  selectResult: (id: string, resultId: number) => void;
  togglePin: (id: string, resultId: number) => void;
  closeResult: (id: string, resultId: number) => Promise<void>;
  /** Drops the sessions of a data source's consoles; they reconnect on next run. */
  disconnectDataSource: (dataSourceId: string) => Promise<void>;
}

export const activeResult = (entry: ConsoleState | undefined): ResultMeta | undefined =>
  entry?.results.find((r) => r.id === entry.activeResultId);

export const isRunning = (entry: ConsoleState | undefined): boolean =>
  !!entry?.results.some((r) => r.status === "running");

/** Where the console's unqualified names resolve: its chosen schema, else the server default. */
export function effectiveSchema(entry: ConsoleState | undefined): string | null {
  if (!entry) return null;
  return entry.schema ?? useDataSources.getState().explorers[entry.dataSourceId]?.server?.defaultSchema ?? null;
}

/**
 * Row storage lives outside React state: pages are appended in place and the
 * grid reads rows by index, so a million rows never get copied or diffed.
 * Keyed by result id; freed when the result's tab or console goes away.
 */
const rowBuffers = new Map<number, Value[][]>();
const rowGetters = new Map<number, (index: number) => Value[] | undefined>();

/** Stable per result, so the grid does not rebind on every render. */
export function rowGetter(resultId: number) {
  let getter = rowGetters.get(resultId);
  if (!getter) {
    getter = (index) => rowBuffers.get(resultId)?.[index];
    rowGetters.set(resultId, getter);
  }
  return getter;
}

function freeRows(resultId: number) {
  rowBuffers.delete(resultId);
  rowGetters.delete(resultId);
}

const STORAGE_KEY = "idedb.consoles.v1";
type PersistedConsole = Pick<ConsoleState, "id" | "dataSourceId" | "table" | "schema" | "sql">;

function restore(): Record<string, ConsoleState> {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as PersistedConsole[];
    return Object.fromEntries(saved.map((c) => [c.id, { ...c, results: [], resultCount: 0 }]));
  } catch {
    return {};
  }
}

let persistTimer = 0;
function persist(consoles: Record<string, ConsoleState>) {
  window.clearTimeout(persistTimer);
  persistTimer = window.setTimeout(() => {
    try {
      const saved: PersistedConsole[] = Object.values(consoles).map(({ id, dataSourceId, table, schema, sql }) => ({
        id,
        dataSourceId,
        table,
        schema,
        sql,
      }));
      localStorage.setItem(STORAGE_KEY, JSON.stringify(saved));
    } catch {
      // Console text not persisted; harmless.
    }
  }, 300);
}

let nextResultId = 0;

export const useConsoles = create<ConsolesState>((set, get) => {
  const patch = (id: string, change: Partial<ConsoleState>) =>
    set((s) => (s.consoles[id] ? { consoles: { ...s.consoles, [id]: { ...s.consoles[id], ...change } } } : {}));

  const patchResult = (id: string, resultId: number, change: Partial<ResultMeta>) =>
    set((s) => {
      const entry = s.consoles[id];
      if (!entry?.results.some((r) => r.id === resultId)) return {};
      const results = entry.results.map((r) => (r.id === resultId ? { ...r, ...change } : r));
      return { consoles: { ...s.consoles, [id]: { ...entry, results } } };
    });

  async function ensureSession(id: string): Promise<SessionId | undefined> {
    const entry = get().consoles[id];
    if (!entry) return undefined;
    if (entry.sessionId !== undefined) return entry.sessionId;

    patch(id, { connecting: true, connectError: undefined });
    try {
      const opened = await openSessionFor(entry.dataSourceId);
      // A new session starts on the server default: re-apply the console's schema.
      const schema = get().consoles[id]?.schema;
      if (opened && schema) {
        await api.setSchema(opened.id, schema).catch((e) => patch(id, { connectError: errorMessage(e) }));
      }
      patch(id, { connecting: false, sessionId: opened?.id });
      // Connecting a console also connects the explorer, as DataGrip does.
      if (opened) void useDataSources.getState().connect(entry.dataSourceId);
      return opened?.id;
    } catch (e) {
      patch(id, { connecting: false, connectError: errorMessage(e) });
      return undefined;
    }
  }

  /** Puts a fresh result in the active tab, or in a new tab when asked or when the active one is pinned. */
  function openResult(id: string, sql: string, newTab: boolean, table?: TableRef): number | undefined {
    const entry = get().consoles[id];
    if (!entry) return undefined;
    const current = activeResult(entry);
    const replace = !newTab && current && !current.pinned ? current : undefined;
    const resultCount = replace ? entry.resultCount : entry.resultCount + 1;
    const result: ResultMeta = {
      id: ++nextResultId,
      title: replace?.title ?? `Result ${resultCount}`,
      pinned: false,
      sql,
      status: "running",
      columns: [],
      rowCount: 0,
      table,
      version: 0,
    };
    if (replace) freeRows(replace.id);
    const results = replace
      ? entry.results.map((r) => (r.id === replace.id ? result : r))
      : [...entry.results, result];
    patch(id, { results, activeResultId: result.id, resultCount });
    return result.id;
  }

  return {
    consoles: restore(),

    create: (dataSourceId, sql = "", table) => {
      const id = crypto.randomUUID();
      set((s) => ({ consoles: { ...s.consoles, [id]: { id, dataSourceId, table, sql, results: [], resultCount: 0 } } }));
      return id;
    },

    remove: async (id) => {
      const entry = get().consoles[id];
      set((s) => {
        const { [id]: _, ...rest } = s.consoles;
        return { consoles: rest };
      });
      for (const result of entry?.results ?? []) freeRows(result.id);
      if (entry?.sessionId !== undefined) await api.closeSession(entry.sessionId).catch(() => {});
    },

    setSql: (id, sql) => patch(id, { sql }),

    runStatement: async (id, sql, options = {}) => {
      const statement = sql.trim();
      if (!statement || isRunning(get().consoles[id])) return undefined;

      const sessionId = await ensureSession(id);
      if (sessionId === undefined) return undefined;
      const resultId = openResult(id, statement, options.newTab ?? false, options.table);
      if (resultId === undefined) return undefined;

      const startedAt = performance.now();
      const rows: Value[][] = [];
      rowBuffers.set(resultId, rows);
      let frame = 0;
      let firstPage = true;
      let hasColumns = false;
      const update = (change: Partial<ResultMeta>) => patchResult(id, resultId, change);

      await api
        .execute(sessionId, statement, (event) => {
          switch (event.kind) {
            case "columns":
              hasColumns = true;
              update({ columns: event.columns });
              break;
            case "rows":
              for (const row of event.rows) rows.push(row);
              if (firstPage) {
                firstPage = false;
                update({ firstPageMs: Math.round(performance.now() - startedAt), rowCount: rows.length });
              } else if (!frame) {
                frame = requestAnimationFrame(() => {
                  frame = 0;
                  update({ rowCount: rows.length });
                });
              }
              break;
            case "done":
              cancelAnimationFrame(frame);
              update({
                status: event.cancelled ? "cancelled" : "done",
                rowCount: rows.length,
                affectedRows: hasColumns ? undefined : event.rowCount,
                elapsedMs: event.elapsedMs,
              });
              patch(id, { inTransaction: event.inTransaction });
              break;
            case "error":
              cancelAnimationFrame(frame);
              update({
                status: "error",
                error: event.message,
                errorPosition: event.position ?? undefined,
                rowCount: rows.length,
              });
              patch(id, { inTransaction: event.inTransaction });
              break;
          }
        })
        .catch((e) => {
          // The session is gone (for example, its data source was disconnected).
          update({ status: "error", error: errorMessage(e) });
          patch(id, { sessionId: undefined, inTransaction: false });
        });
      // Whatever ran may have created or dropped objects the editors check against.
      const dataSourceId = get().consoles[id]?.dataSourceId;
      if (dataSourceId) invalidateChecks(dataSourceId);

      return get().consoles[id]?.results.find((r) => r.id === resultId);
    },

    cancel: async (id) => {
      const entry = get().consoles[id];
      if (entry?.sessionId === undefined || !isRunning(entry)) return;
      await api.cancel(entry.sessionId);
    },

    mutateRows: (id, resultId, change) => {
      const rows = rowBuffers.get(resultId);
      const result = get().consoles[id]?.results.find((r) => r.id === resultId);
      if (!rows || !result) return;
      change(rows);
      patchResult(id, resultId, { rowCount: rows.length, version: result.version + 1 });
    },

    setInTransaction: (id, inTransaction) => patch(id, { inTransaction }),

    setTableFilter: (id, filter) => {
      const table = get().consoles[id]?.table;
      if (table) patch(id, { table: { ...table, ...filter } });
    },

    setSchema: async (id, schema) => {
      const entry = get().consoles[id];
      if (!entry) return;
      patch(id, { schema, connectError: undefined });
      // Without a session there is nothing to switch yet: the schema applies on connect.
      const target = schema ?? useDataSources.getState().explorers[entry.dataSourceId]?.server?.defaultSchema;
      if (entry.sessionId === undefined || !target) return;
      await api.setSchema(entry.sessionId, target).catch((e) => patch(id, { connectError: errorMessage(e) }));
    },

    selectResult: (id, resultId) => patch(id, { activeResultId: resultId }),

    togglePin: (id, resultId) => {
      const result = get().consoles[id]?.results.find((r) => r.id === resultId);
      if (result) patchResult(id, resultId, { pinned: !result.pinned });
    },

    closeResult: async (id, resultId) => {
      const entry = get().consoles[id];
      const index = entry?.results.findIndex((r) => r.id === resultId) ?? -1;
      if (!entry || index < 0) return;
      if (entry.results[index].status === "running") await get().cancel(id);

      const results = entry.results.filter((r) => r.id !== resultId);
      const activeResultId =
        entry.activeResultId === resultId ? results[Math.min(index, results.length - 1)]?.id : entry.activeResultId;
      patch(id, { results, activeResultId });
      freeRows(resultId);
    },

    disconnectDataSource: async (dataSourceId) => {
      const affected = Object.values(get().consoles).filter(
        (c) => c.dataSourceId === dataSourceId && c.sessionId !== undefined,
      );
      for (const c of affected) patch(c.id, { sessionId: undefined, inTransaction: false });
      await Promise.all(affected.map((c) => api.closeSession(c.sessionId!).catch(() => {})));
    },
  };
});

useConsoles.subscribe((s) => persist(s.consoles));
