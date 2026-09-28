import { ask } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { api, errorMessage, type Column, type SessionId, type TableRef, type Value } from "./api";
import { invalidateChecks } from "./checkRevisions";
import { openSessionFor, useDataSources } from "./dataSources";
import { moreAfter, useFetchSettings, type MoreRows } from "./fetching";
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
  /** Rows past the loaded ones: none, open on the session, or released. */
  more: MoreRows;
  /** A fetch of the next page, or of all the rest, is running. */
  fetching?: "page" | "all";
  /** Why the last fetch failed, e.g. the rest was closed on the session. */
  fetchError?: string;
  /** How long the last fetch of more rows took. */
  fetchMs?: number;
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
   * to `undefined` when nothing ran (busy console, no session, or the user
   * kept the pending edits the run would have replaced). `confirmed`: the
   * caller already asked with `confirmReplace`.
   */
  runStatement: (
    id: string,
    sql: string,
    options?: { newTab?: boolean; table?: TableRef; confirmed?: boolean },
  ) => Promise<ResultMeta | undefined>;
  /**
   * Whether a run may replace the result it would reuse: asks first when
   * that result has data editor changes not yet submitted.
   */
  confirmReplace: (id: string, options?: { newTab?: boolean }) => Promise<boolean>;
  /** Changes a result's loaded rows in place and repaints its grid. */
  mutateRows: (id: string, resultId: number, change: (rows: Value[][]) => void) => void;
  /** Records whether the console's session has a user transaction open (after a data editor submit). */
  setInTransaction: (id: string, inTransaction: boolean) => void;
  setTableFilter: (id: string, filter: TableFilter) => void;
  /** Switches the console's current schema; `undefined` goes back to the server default. */
  setSchema: (id: string, schema: string | undefined) => Promise<void>;
  cancel: (id: string) => Promise<void>;
  /** Loads the next page of a result's open rest, or all of it. */
  fetchMore: (id: string, resultId: number, all?: boolean) => Promise<void>;
  /** Releases the console's open result without loading the rest. */
  closeCursor: (id: string) => Promise<void>;
  /**
   * Call before the console's session does anything but read on (a data
   * editor submit, a schema switch): the session closes its open result
   * then, so the result stops offering more rows.
   */
  releaseOpenResults: (id: string) => void;
  selectResult: (id: string, resultId: number) => void;
  togglePin: (id: string, resultId: number) => void;
  closeResult: (id: string, resultId: number) => Promise<void>;
  /** Drops the sessions of a data source's consoles; they reconnect on next run. */
  disconnectDataSource: (dataSourceId: string) => Promise<void>;
}

export const activeResult = (entry: ConsoleState | undefined): ResultMeta | undefined =>
  entry?.results.find((r) => r.id === entry.activeResultId);

/** Whether the console's session is busy: running a statement or fetching more rows. */
export const isRunning = (entry: ConsoleState | undefined): boolean =>
  !!entry?.results.some((r) => r.status === "running" || r.fetching !== undefined);

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

/** Session opens in flight, by console. */
const sessionOpens = new Map<string, Promise<SessionId | undefined>>();

/**
 * Counts a result's data editor changes not yet submitted. The grid module
 * installs it (it depends on this one, not the other way around).
 */
let pendingChangesOf: (resultId: number) => number = () => 0;

export function setPendingChangesProbe(probe: (resultId: number) => number) {
  pendingChangesOf = probe;
}

/**
 * Hooks around every statement a console runs, installed by transactions.ts
 * (manual transaction mode). Kept as a seam so this module does not depend
 * on it.
 */
export interface ExecutionHooks {
  /**
   * Runs on the session right before `sql`, e.g. to open a transaction.
   * Resolves to an error message to fail the run instead of executing it.
   * `tableLoad`: the data editor loading a table, not the user's SQL.
   */
  before: (consoleId: string, sessionId: SessionId, sql: string, tableLoad: boolean) => Promise<string | undefined>;
  /** After `sql` ended, with whether a user transaction was open around it. */
  after: (
    consoleId: string,
    sql: string,
    outcome: { wasOpen: boolean; nowOpen: boolean; error?: string; sessionLost: boolean },
  ) => void;
}

let executionHooks: ExecutionHooks = { before: async () => undefined, after: () => {} };

export function setExecutionHooks(hooks: ExecutionHooks) {
  executionHooks = hooks;
}

/** The one place that asks before unsubmitted changes are thrown away. */
async function confirmDiscard(count: number): Promise<boolean> {
  return ask("They have not been submitted and will be lost.", {
    title: `Discard ${count} pending ${count === 1 ? "change" : "changes"}?`,
    kind: "warning",
    okLabel: "Discard",
    cancelLabel: "Keep Editing",
  });
}

/** The result a run would reuse: the active tab, unless asked for a new one or pinned. */
function replacedResult(entry: ConsoleState | undefined, newTab: boolean): ResultMeta | undefined {
  const current = activeResult(entry);
  return !newTab && current && !current.pinned ? current : undefined;
}

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

  /** The console's session, opening it on first use. Runs started while it opens share the one open. */
  function ensureSession(id: string): Promise<SessionId | undefined> {
    const entry = get().consoles[id];
    if (!entry) return Promise.resolve(undefined);
    if (entry.sessionId !== undefined) return Promise.resolve(entry.sessionId);
    let pending = sessionOpens.get(id);
    if (!pending) {
      pending = openSession(id, entry.dataSourceId).finally(() => sessionOpens.delete(id));
      sessionOpens.set(id, pending);
    }
    return pending;
  }

  async function openSession(id: string, dataSourceId: string): Promise<SessionId | undefined> {
    patch(id, { connecting: true, connectError: undefined });
    try {
      const opened = await openSessionFor(dataSourceId);
      // Nobody to hand it to: the console was closed or its data source deleted meanwhile.
      const gone = !get().consoles[id] || !useDataSources.getState().sources.some((s) => s.id === dataSourceId);
      if (opened && gone) {
        await api.closeSession(opened.id).catch(() => {});
        return undefined;
      }
      // A new session starts on the server default: re-apply the console's schema.
      const schema = get().consoles[id]?.schema;
      if (opened && schema) {
        await api.setSchema(opened.id, schema).catch((e) => patch(id, { connectError: errorMessage(e) }));
      }
      patch(id, { connecting: false, sessionId: opened?.id, inTransaction: false });
      // Connecting a console also connects the explorer, as DataGrip does.
      if (opened) void useDataSources.getState().connect(dataSourceId);
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
    const replace = replacedResult(entry, newTab);
    const resultCount = replace ? entry.resultCount : entry.resultCount + 1;
    const result: ResultMeta = {
      id: ++nextResultId,
      title: replace?.title ?? `Result ${resultCount}`,
      pinned: false,
      sql,
      status: "running",
      columns: [],
      rowCount: 0,
      more: "none",
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

    confirmReplace: async (id, options = {}) => {
      const replaced = replacedResult(get().consoles[id], options.newTab ?? false);
      const pending = replaced ? pendingChangesOf(replaced.id) : 0;
      return pending === 0 || confirmDiscard(pending);
    },

    runStatement: async (id, sql, options = {}) => {
      const statement = sql.trim();
      if (!statement || isRunning(get().consoles[id])) return undefined;
      if (!options.confirmed && !(await get().confirmReplace(id, options))) return undefined;

      const sessionId = await ensureSession(id);
      // Another run that shared the session open may have started meanwhile.
      if (sessionId === undefined || isRunning(get().consoles[id])) return undefined;
      // Running anything closes the session's open result.
      get().releaseOpenResults(id);
      const resultId = openResult(id, statement, options.newTab ?? false, options.table);
      if (resultId === undefined) return undefined;

      const refused = await executionHooks.before(id, sessionId, statement, options.table !== undefined);
      if (refused) {
        patchResult(id, resultId, { status: "error", error: refused });
        return get().consoles[id]?.results.find((r) => r.id === resultId);
      }
      const wasOpen = get().consoles[id]?.inTransaction ?? false;
      let sessionLost = false;

      const startedAt = performance.now();
      const rows: Value[][] = [];
      rowBuffers.set(resultId, rows);
      let frame = 0;
      let firstPage = true;
      let hasColumns = false;
      const update = (change: Partial<ResultMeta>) => patchResult(id, resultId, change);

      await api
        .execute(sessionId, statement, useFetchSettings.getState().pageSize, (event) => {
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
                more: moreAfter(event),
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
          sessionLost = true;
          update({ status: "error", error: errorMessage(e) });
          patch(id, { sessionId: undefined, inTransaction: false });
        });
      // Whatever ran may have created or dropped objects the editors check against.
      const dataSourceId = get().consoles[id]?.dataSourceId;
      if (dataSourceId) invalidateChecks(dataSourceId);

      const finished = get().consoles[id]?.results.find((r) => r.id === resultId);
      executionHooks.after(id, statement, {
        wasOpen,
        nowOpen: get().consoles[id]?.inTransaction ?? false,
        error: finished?.status === "error" ? finished.error : undefined,
        sessionLost,
      });
      return finished;
    },

    cancel: async (id) => {
      const entry = get().consoles[id];
      if (entry?.sessionId === undefined || !isRunning(entry)) return;
      await api.cancel(entry.sessionId);
    },

    fetchMore: async (id, resultId, all = false) => {
      const entry = get().consoles[id];
      const result = entry?.results.find((r) => r.id === resultId);
      const rows = rowBuffers.get(resultId);
      const sessionId = entry?.sessionId;
      if (!result || !rows || result.more !== "open" || isRunning(entry) || sessionId === undefined) return;

      const startedAt = performance.now();
      let frame = 0;
      const update = (change: Partial<ResultMeta>) => patchResult(id, resultId, change);
      update({ fetching: all ? "all" : "page", fetchError: undefined });
      const pageSize = useFetchSettings.getState().pageSize;
      await api
        .fetchMore(sessionId, all ? null : pageSize, (event) => {
          switch (event.kind) {
            case "rows":
              // Appended in place, like the first page: the grid reads rows by index.
              for (const row of event.rows) rows.push(row);
              if (!frame) {
                frame = requestAnimationFrame(() => {
                  frame = 0;
                  update({ rowCount: rows.length });
                });
              }
              break;
            case "done":
              cancelAnimationFrame(frame);
              update({
                fetching: undefined,
                rowCount: rows.length,
                more: moreAfter(event),
                fetchMs: Math.round(performance.now() - startedAt),
              });
              patch(id, { inTransaction: event.inTransaction });
              break;
            case "error":
              cancelAnimationFrame(frame);
              update({ fetching: undefined, rowCount: rows.length, more: "closed", fetchError: event.message });
              patch(id, { inTransaction: event.inTransaction });
              break;
            case "columns":
              break;
          }
        })
        .catch((e) => {
          cancelAnimationFrame(frame);
          update({ fetching: undefined, rowCount: rows.length, more: "closed", fetchError: errorMessage(e) });
        });
    },

    closeCursor: async (id) => {
      const entry = get().consoles[id];
      if (entry?.sessionId === undefined || isRunning(entry) || !entry.results.some((r) => r.more === "open")) return;
      get().releaseOpenResults(id);
      await api.closeResult(entry.sessionId).catch(() => {});
    },

    releaseOpenResults: (id) => {
      for (const result of get().consoles[id]?.results ?? []) {
        if (result.more === "open") patchResult(id, result.id, { more: "closed" });
      }
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
      get().releaseOpenResults(id);
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
      const pending = pendingChangesOf(resultId);
      if (pending > 0 && !(await confirmDiscard(pending))) return;
      const closing = entry.results[index];
      if (closing.status === "running" || closing.fetching) await get().cancel(id);
      // Nobody will read the rest: release what the session holds for it.
      else if (closing.more === "open") await get().closeCursor(id);

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
