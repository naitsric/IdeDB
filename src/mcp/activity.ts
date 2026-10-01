import type { AuditEntry, AuditFilter, Decision } from "./api";

/**
 * The Activity view's log: the audit rows matching the user's filters,
 * newest first. Pure, so the store and the tests share it.
 *
 * It is put together from three sources, each complete over a range of
 * ids that ends at the newest row:
 *
 * - the store's live ring (the last rows recorded, all of them; see
 *   `AUDIT_RING`), filtered here, so a new filter shows results at once;
 * - pages of history from `mcp_audit_list`, filtered by the store with the
 *   same conditions, each one picking up where the oldest row shown ends;
 * - rows recorded since, from `mcp://event`, kept when they match.
 *
 * Their union has no gaps, so the oldest row shown is the cursor for the
 * next page. Rows don't change once recorded; one seen twice is kept once.
 */

/** What the Activity view shows. Unset fields match everything. */
export interface ActivityFilter {
  clientId: string | null;
  dataSourceId: string | null;
  decision: Decision | null;
  /** Contained in the SQL; empty matches every row, with or without SQL. */
  search: string;
}

export const NO_FILTER: ActivityFilter = { clientId: null, dataSourceId: null, decision: null, search: "" };

/** Rows asked for per page of history. */
export const PAGE_SIZE = 200;

/**
 * Rows kept in memory at most, as many as the store retains. Past it the
 * oldest go, and Load Older brings them back.
 */
export const MAX_ENTRIES = 20_000;

export interface ActivityLog {
  /** Newest first, each id once. */
  entries: AuditEntry[];
  /** History was asked for under the current filter (not just the ring's rows). */
  started: boolean;
  /** A page of history is on its way. */
  loading: boolean;
  /** No older row matches: the last page came back short. */
  exhausted: boolean;
  /** Why the last page couldn't be read. */
  error: string | null;
}

export const emptyLog: ActivityLog = { entries: [], started: false, loading: false, exhausted: false, error: null };

export function isFiltered(filter: ActivityFilter): boolean {
  return filter.clientId !== null || filter.dataSourceId !== null || filter.decision !== null || filter.search !== "";
}

/** Lowercases A–Z only, like SQLite's `lower()`, which the store's search uses. */
const asciiLower = (text: string) => text.replace(/[A-Z]+/g, (s) => s.toLowerCase());

/**
 * Whether a row belongs in the view: the conditions `mcp_audit_list`
 * applies, applied here to rows that come from the ring and from events.
 */
export function matches(entry: AuditEntry, filter: ActivityFilter): boolean {
  if (filter.clientId !== null && entry.clientId !== filter.clientId) return false;
  if (filter.dataSourceId !== null && entry.dataSourceId !== filter.dataSourceId) return false;
  if (filter.decision !== null && entry.decision !== filter.decision) return false;
  if (filter.search !== "") {
    if (entry.sql === null || !asciiLower(entry.sql).includes(asciiLower(filter.search))) return false;
  }
  return true;
}

/** The store's filter for a page of history: rows older than `beforeId`, when given. */
export function toAuditFilter(filter: ActivityFilter, beforeId: number | null, limit = PAGE_SIZE): AuditFilter {
  return {
    clientId: filter.clientId,
    dataSourceId: filter.dataSourceId,
    decision: filter.decision,
    search: filter.search === "" ? null : filter.search,
    beforeId,
    limit,
  };
}

/** Two lists of rows, newest first, as one: newest first, each id once. Linear. */
export function mergeEntries(a: readonly AuditEntry[], b: readonly AuditEntry[]): AuditEntry[] {
  if (b.length === 0) return a as AuditEntry[];
  if (a.length === 0) return b as AuditEntry[];
  const merged: AuditEntry[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    const next = j >= b.length || (i < a.length && a[i].id >= b[j].id) ? a[i++] : b[j++];
    if (merged.length === 0 || merged[merged.length - 1].id !== next.id) merged.push(next);
  }
  return merged;
}

/** Keeps the newest {@link MAX_ENTRIES}; what goes can be loaded again. */
function capped(log: ActivityLog): ActivityLog {
  if (log.entries.length <= MAX_ENTRIES) return log;
  return { ...log, entries: log.entries.slice(0, MAX_ENTRIES), exhausted: false };
}

/** A log for a new filter: what the ring has that matches, until history arrives. */
export function startLog(ring: readonly AuditEntry[], filter: ActivityFilter): ActivityLog {
  return { ...emptyLog, entries: ring.filter((e) => matches(e, filter)), started: true, loading: true };
}

/** Adds a page of history asked for with `limit`. A short page means there is nothing older. */
export function addPage(log: ActivityLog, page: readonly AuditEntry[], limit = PAGE_SIZE): ActivityLog {
  return capped({
    ...log,
    entries: mergeEntries(log.entries, page),
    loading: false,
    error: null,
    exhausted: page.length < limit,
  });
}

/**
 * Adds rows recorded since (from events, or the store's newest), those that
 * match. Until the log starts there is nothing to add to: starting reads
 * the ring.
 */
export function addLive(log: ActivityLog, entries: readonly AuditEntry[], filter: ActivityFilter): ActivityLog {
  const matching = log.started ? entries.filter((e) => matches(e, filter)) : [];
  if (matching.length === 0) return log;
  // The usual case: one row, newer than any shown.
  const newest = log.entries[0];
  const fresh =
    matching.length === 1 && (!newest || matching[0].id > newest.id)
      ? [matching[0], ...log.entries]
      : mergeEntries(log.entries, [...matching].sort((x, y) => y.id - x.id));
  return capped({ ...log, entries: fresh });
}

/**
 * Where the next page of history starts: before the oldest row shown, or
 * `undefined` when there is nothing older to load.
 */
export function olderCursor(log: ActivityLog): number | null | undefined {
  if (log.exhausted) return undefined;
  return log.entries.at(-1)?.id ?? null;
}

/** Whether Load Older can run now. */
export function canLoadOlder(log: ActivityLog): boolean {
  return log.started && !log.loading && !log.exhausted;
}

/**
 * Rows newer than `id`: those that arrived above the one the user last
 * saw at the top. `null` (nothing seen yet) counts none.
 */
export function newerThan(entries: readonly AuditEntry[], id: number | null): number {
  if (id === null) return 0;
  // Newest first: count up to the first row not newer.
  let count = 0;
  while (count < entries.length && entries[count].id > id) count++;
  return count;
}

// Rows on screen

/** The first and past-the-last index of the rows to render. */
export interface RowRange {
  start: number;
  end: number;
}

/**
 * Which of `count` fixed-height rows a viewport shows, plus `overscan`
 * rows either side so fast scrolling doesn't flash empty space.
 */
export function visibleRange(
  scrollTop: number,
  viewport: number,
  rowHeight: number,
  count: number,
  overscan = 8,
): RowRange {
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const last = Math.ceil((Math.max(0, scrollTop) + viewport) / rowHeight);
  return { start: Math.max(0, first - overscan), end: Math.min(count, last + overscan) };
}

/** The scroll position that brings row `index` into view, moving as little as possible. */
export function revealRow(index: number, rowHeight: number, scrollTop: number, viewport: number): number {
  const top = index * rowHeight;
  const bottom = top + rowHeight;
  if (top < scrollTop) return top;
  if (bottom > scrollTop + viewport) return Math.max(0, bottom - viewport);
  return scrollTop;
}

/**
 * The row `step` rows away from the selected one, kept in the list. With
 * none selected (or the selected one filtered out), moving starts above
 * the first row.
 */
export function stepSelection(entries: readonly AuditEntry[], selectedId: number | null, step: number): number | null {
  if (entries.length === 0) return null;
  const index = selectedId === null ? -1 : entries.findIndex((e) => e.id === selectedId);
  const next = index === -1 ? Math.max(0, step - 1) : index + step;
  return entries[Math.max(0, Math.min(entries.length - 1, next))].id;
}

// What a row says

const timeOfDay = new Intl.DateTimeFormat("en-US", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });
const dayAndTime = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});
const dayOfYear = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", year: "numeric" });
const fullTime = new Intl.DateTimeFormat("en-US", {
  weekday: "short",
  month: "short",
  day: "numeric",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  fractionalSecondDigits: 3,
  hourCycle: "h23",
  timeZoneName: "short",
});

const sameDay = (a: Date, b: Date) =>
  a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();

/** When a row ran, short: `14:04:31` today, `Sep 28, 14:04` this year, `Sep 28, 2025` before. */
export function formatLogTime(iso: string, now: number): string {
  const date = new Date(iso);
  const today = new Date(now);
  if (sameDay(date, today)) return timeOfDay.format(date);
  return (date.getFullYear() === today.getFullYear() ? dayAndTime : dayOfYear).format(date);
}

/** The full timestamp, to the millisecond and with the time zone, for tooltips and the detail. */
export function formatFullTime(iso: string): string {
  return fullTime.format(new Date(iso));
}

/** Characters of SQL a row's preview starts from; the row cuts it far sooner. */
const PREVIEW_CHARS = 400;

/**
 * The SQL on one line, for a row: line breaks and indentation collapsed,
 * as Query History shows it, so a statement whose first line is just
 * `with recent as (` still says what it does.
 */
export function oneLine(sql: string): string {
  return sql.trimStart().slice(0, PREVIEW_CHARS).replace(/\s+/g, " ").trim();
}

/**
 * Why a row's SQL can't open in a console, or `null` when it can: the call
 * must have run SQL on a data source that still exists.
 */
export function consoleBlocker(
  entry: Pick<AuditEntry, "sql" | "dataSourceId" | "dataSourceName">,
  sourceExists: boolean,
): string | null {
  if (entry.sql === null) return "This call ran no SQL.";
  if (entry.dataSourceId === null) return "The call named no data source IdeDB knows.";
  if (!sourceExists) return `The data source "${entry.dataSourceName ?? entry.dataSourceId}" no longer exists.`;
  return null;
}

/** What the tools that run no SQL did. */
const CALLED: Record<string, string> = {
  list_connections: "Listed the data sources it may use",
  list_schemas: "Listed schemas",
  list_tables: "Listed tables",
  describe_table: "Described a table",
};

/** A row's second line when the call ran no SQL: why it failed, or what it did. */
export function callSummary(entry: Pick<AuditEntry, "tool" | "error">): string {
  return entry.error ?? CALLED[entry.tool] ?? `Called ${entry.tool}`;
}

/** What each tool's count counts. */
const COUNTED: Record<string, [one: string, many: string]> = {
  list_connections: ["connection", "connections"],
  list_schemas: ["schema", "schemas"],
  list_tables: ["table", "tables"],
  describe_table: ["column", "columns"],
};

/** `12 rows`, `1 table`, `200+ rows` when the result was cut short; `null` without a count. */
export function countLabel(entry: Pick<AuditEntry, "tool" | "rowCount" | "truncated">): string | null {
  if (entry.rowCount === null) return null;
  const [one, many] = COUNTED[entry.tool] ?? ["row", "rows"];
  const n = entry.rowCount;
  return `${n.toLocaleString("en-US")}${entry.truncated ? "+" : ""} ${n === 1 && !entry.truncated ? one : many}`;
}
