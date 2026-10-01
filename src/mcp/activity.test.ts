import { describe, expect, it } from "vitest";
import {
  addLive,
  addPage,
  callSummary,
  canLoadOlder,
  consoleBlocker,
  countLabel,
  emptyLog,
  oneLine,
  formatFullTime,
  formatLogTime,
  isFiltered,
  matches,
  MAX_ENTRIES,
  mergeEntries,
  newerThan,
  NO_FILTER,
  olderCursor,
  revealRow,
  startLog,
  stepSelection,
  toAuditFilter,
  visibleRange,
  type ActivityFilter,
  type ActivityLog,
} from "./activity";
import type { AuditEntry } from "./api";

const entry = (id: number, extra: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  at: "2026-10-01T12:00:00.000Z",
  clientId: "c1",
  clientName: "claude-code",
  clientInfoName: null,
  clientInfoVersion: null,
  protocolVersion: "2026-07-28",
  transport: "http",
  sessionKey: null,
  tool: "query",
  dataSourceId: "pg",
  dataSourceName: "shop",
  sql: `select ${id}`,
  sqlTruncated: false,
  statementKind: "SELECT",
  reason: null,
  decision: "allowed",
  approvalWaitMs: null,
  elapsedMs: 3,
  rowCount: 1,
  truncated: false,
  error: null,
  ...extra,
});

const ids = (entries: readonly AuditEntry[]) => entries.map((e) => e.id);
const filter = (extra: Partial<ActivityFilter>): ActivityFilter => ({ ...NO_FILTER, ...extra });
/** Newest first, like the store returns them. */
const range = (from: number, to: number, extra: (id: number) => Partial<AuditEntry> = () => ({})) =>
  Array.from({ length: from - to + 1 }, (_, i) => entry(from - i, extra(from - i)));

describe("matches", () => {
  it("applies each filter the store applies", () => {
    const e = entry(1);
    expect(matches(e, NO_FILTER)).toBe(true);
    expect(matches(e, filter({ clientId: "c1" }))).toBe(true);
    expect(matches(e, filter({ clientId: "c2" }))).toBe(false);
    expect(matches(e, filter({ dataSourceId: "pg" }))).toBe(true);
    expect(matches(e, filter({ dataSourceId: "lite" }))).toBe(false);
    expect(matches(e, filter({ decision: "allowed" }))).toBe(true);
    expect(matches(e, filter({ decision: "denied" }))).toBe(false);
    expect(matches(e, filter({ clientId: "c1", decision: "denied" }))).toBe(false);
  });

  it("searches the SQL ignoring ASCII case only, like SQLite's lower()", () => {
    const e = entry(1, { sql: "SELECT * FROM Orders WHERE note = 'ÉTÉ'" });
    expect(matches(e, filter({ search: "from orders" }))).toBe(true);
    expect(matches(e, filter({ search: "ORDERS where" }))).toBe(true);
    expect(matches(e, filter({ search: "'ÉTÉ'" }))).toBe(true);
    // The store lowercases A–Z only, so neither does this.
    expect(matches(e, filter({ search: "'été'" }))).toBe(false);
    expect(matches(e, filter({ search: "customers" }))).toBe(false);
  });

  it("leaves out calls without SQL when searching, and keeps them otherwise", () => {
    const listed = entry(1, { tool: "list_tables", sql: null, statementKind: null });
    expect(matches(listed, filter({ search: "select" }))).toBe(false);
    expect(matches(listed, NO_FILTER)).toBe(true);
  });

  it("tells a filtered view", () => {
    expect(isFiltered(NO_FILTER)).toBe(false);
    expect(isFiltered(filter({ search: "x" }))).toBe(true);
    expect(isFiltered(filter({ decision: "timeout" }))).toBe(true);
  });
});

describe("toAuditFilter", () => {
  it("asks the store for the same rows, older than the cursor", () => {
    expect(toAuditFilter(filter({ clientId: "c1", search: "orders" }), 41, 50)).toEqual({
      clientId: "c1",
      dataSourceId: null,
      decision: null,
      search: "orders",
      beforeId: 41,
      limit: 50,
    });
    expect(toAuditFilter(NO_FILTER, null).search).toBeNull();
  });
});

describe("mergeEntries", () => {
  it("keeps the newest first, each id once", () => {
    const a = [entry(9), entry(7), entry(4)];
    const b = [entry(8), entry(7), entry(5), entry(4), entry(1)];
    expect(ids(mergeEntries(a, b))).toEqual([9, 8, 7, 5, 4, 1]);
    expect(ids(mergeEntries([], b))).toEqual([8, 7, 5, 4, 1]);
    expect(ids(mergeEntries(a, []))).toEqual([9, 7, 4]);
  });
});

describe("the log", () => {
  it("starts with what the ring has that matches, and loads the newest page", () => {
    const ring = [entry(5, { decision: "denied" }), entry(4), entry(3, { decision: "denied" })];
    const log = startLog(ring, filter({ decision: "denied" }));
    expect(ids(log.entries)).toEqual([5, 3]);
    expect(log).toMatchObject({ started: true, loading: true, exhausted: false });
  });

  it("adds pages of history, and knows a short page is the last", () => {
    let log = startLog([], NO_FILTER);
    log = addPage(log, range(300, 201), 100);
    expect(log).toMatchObject({ loading: false, exhausted: false });
    expect(olderCursor(log)).toBe(201);
    log = addPage(log, range(200, 151), 100);
    expect(log.exhausted).toBe(true);
    expect(olderCursor(log)).toBeUndefined();
    expect(log.entries).toHaveLength(150);
  });

  it("puts rows from the ring, history and events together without gaps or repeats", () => {
    // The server has rows 1..1000; odd ones were denied.
    const denied = filter({ decision: "denied" });
    const decision = (id: number) => ({ decision: id % 2 ? ("denied" as const) : ("allowed" as const) });
    const ring = range(1000, 501, decision);
    const server = (beforeId: number | null, limit: number) =>
      range(beforeId === null ? 1000 : beforeId - 1, 1, decision)
        .filter((e) => e.decision === "denied")
        .slice(0, limit);

    // The ring alone already has 250 matches, more than the first page.
    let log = startLog(ring, denied);
    log = addPage(log, server(null, 100), 100);
    expect(log.entries).toHaveLength(250);
    // The next page starts below the oldest match the ring had.
    expect(olderCursor(log)).toBe(501);
    log = addPage(log, server(501, 100), 100);
    log = addPage(log, server(olderCursor(log)!, 100), 100);
    log = addPage(log, server(olderCursor(log)!, 100), 100);
    // A new denied row arrives meanwhile; an allowed one is left out.
    log = addLive(log, [entry(1001, decision(1001)), entry(1002, decision(1002))], denied);

    expect(log.exhausted).toBe(true);
    expect(ids(log.entries)).toEqual(range(1001, 1, decision).filter((e) => e.decision === "denied").map((e) => e.id));
  });

  it("puts live rows first, keeps them once and leaves out what doesn't match", () => {
    let log = addPage(startLog([], NO_FILTER), [entry(3), entry(2)], 100);
    log = addLive(log, [entry(4)], NO_FILTER);
    log = addLive(log, [entry(4)], NO_FILTER);
    log = addLive(log, [entry(5, { clientId: "c2" })], filter({ clientId: "c1" }));
    expect(ids(log.entries)).toEqual([4, 3, 2]);
    // Several at once, in any order (a reload of the newest rows).
    log = addLive(log, [entry(6), entry(8), entry(7)], NO_FILTER);
    expect(ids(log.entries)).toEqual([8, 7, 6, 4, 3, 2]);
    expect(addLive(log, [entry(9, { decision: "denied" })], filter({ decision: "allowed" }))).toBe(log);
  });

  it("holds nothing before it starts: starting reads the ring", () => {
    expect(addLive(emptyLog, [entry(1)], NO_FILTER)).toBe(emptyLog);
  });

  it("keeps the newest rows past its size, and lets the rest load again", () => {
    let log: ActivityLog = { ...emptyLog, started: true, exhausted: true, entries: range(MAX_ENTRIES, 1) };
    log = addLive(log, [entry(MAX_ENTRIES + 1)], NO_FILTER);
    expect(log.entries).toHaveLength(MAX_ENTRIES);
    expect(log.entries[0].id).toBe(MAX_ENTRIES + 1);
    expect(log.exhausted).toBe(false);
    expect(olderCursor(log)).toBe(2);
  });

  it("loads older only when started, idle and not at the end", () => {
    const idle: ActivityLog = { ...emptyLog, started: true, entries: [entry(1)] };
    expect(canLoadOlder(idle)).toBe(true);
    expect(canLoadOlder({ ...idle, loading: true })).toBe(false);
    expect(canLoadOlder({ ...idle, exhausted: true })).toBe(false);
    expect(canLoadOlder(emptyLog)).toBe(false);
    // A failed page is retried from where it stopped.
    expect(canLoadOlder({ ...idle, error: "disk I/O error" })).toBe(true);
    // Nothing loaded yet: from the newest.
    expect(olderCursor({ ...idle, entries: [] })).toBeNull();
  });
});

describe("new rows", () => {
  it("counts the rows above the last one seen", () => {
    const entries = range(10, 1);
    expect(newerThan(entries, 7)).toBe(3);
    expect(newerThan(entries, 10)).toBe(0);
    expect(newerThan(entries, 0)).toBe(10);
    expect(newerThan(entries, null)).toBe(0);
    expect(newerThan([], 3)).toBe(0);
  });
});

describe("rows on screen", () => {
  it("renders the rows in view and a few either side", () => {
    expect(visibleRange(0, 440, 44, 1000, 5)).toEqual({ start: 0, end: 15 });
    expect(visibleRange(4400, 440, 44, 1000, 5)).toEqual({ start: 95, end: 115 });
    expect(visibleRange(4400, 440, 44, 104, 5)).toEqual({ start: 95, end: 104 });
    expect(visibleRange(0, 440, 44, 0)).toEqual({ start: 0, end: 0 });
  });

  it("scrolls as little as it takes to show a row", () => {
    expect(revealRow(5, 44, 0, 440)).toBe(0);
    expect(revealRow(12, 44, 0, 440)).toBe(13 * 44 - 440);
    expect(revealRow(2, 44, 440, 440)).toBe(88);
  });

  it("moves the selection within the list", () => {
    const entries = range(5, 1);
    expect(stepSelection(entries, null, 1)).toBe(5);
    expect(stepSelection(entries, null, -1)).toBe(5);
    expect(stepSelection(entries, null, entries.length)).toBe(1);
    expect(stepSelection(entries, 5, 1)).toBe(4);
    expect(stepSelection(entries, 1, 1)).toBe(1);
    expect(stepSelection(entries, 3, -10)).toBe(5);
    expect(stepSelection(entries, 99, 1)).toBe(5);
    expect(stepSelection([], null, 1)).toBeNull();
  });
});

describe("what a row says", () => {
  // Local times, so the day boundaries hold in any time zone.
  const now = new Date(2026, 9, 1, 15, 30).getTime();
  const at = (...parts: [number, number, number, number, number, number]) => new Date(...parts).toISOString();

  it("says when, as short as it can", () => {
    expect(formatLogTime(at(2026, 9, 1, 14, 4, 31), now)).toBe("14:04:31");
    expect(formatLogTime(at(2026, 8, 28, 9, 5, 0), now)).toBe("Sep 28, 09:05");
    expect(formatLogTime(at(2025, 8, 28, 9, 5, 0), now)).toBe("Sep 28, 2025");
  });

  it("gives the full time to the millisecond", () => {
    const full = formatFullTime(new Date(2026, 9, 1, 14, 4, 31, 120).toISOString());
    expect(full).toContain("Thu, Oct 1, 2026");
    expect(full).toContain("14:04:31.120");
  });

  it("shows the SQL on one line", () => {
    expect(oneLine("with recent as (\n  select * from orders\n)\nselect count(*) from recent")).toBe(
      "with recent as ( select * from orders ) select count(*) from recent",
    );
    expect(oneLine("\n\n  select   1  ")).toBe("select 1");
    // A 100 KiB statement isn't copied whole into every row.
    expect(oneLine(`select ${"x, ".repeat(50_000)}1`).length).toBeLessThanOrEqual(400);
  });

  it("counts what the tool returned", () => {
    expect(countLabel({ tool: "query", rowCount: 1, truncated: false })).toBe("1 row");
    expect(countLabel({ tool: "query", rowCount: 1200, truncated: false })).toBe("1,200 rows");
    expect(countLabel({ tool: "query", rowCount: 200, truncated: true })).toBe("200+ rows");
    expect(countLabel({ tool: "list_tables", rowCount: 12, truncated: false })).toBe("12 tables");
    expect(countLabel({ tool: "describe_table", rowCount: 1, truncated: false })).toBe("1 column");
    expect(countLabel({ tool: "execute", rowCount: null, truncated: false })).toBeNull();
  });

  it("says what a call without SQL did", () => {
    expect(callSummary({ tool: "list_tables", error: null })).toBe("Listed tables");
    expect(callSummary({ tool: "list_tables", error: "No access to 'crm'" })).toBe("No access to 'crm'");
    expect(callSummary({ tool: "future_tool", error: null })).toBe("Called future_tool");
  });

  it("opens in a console only SQL on a data source that still exists", () => {
    expect(consoleBlocker(entry(1), true)).toBeNull();
    expect(consoleBlocker(entry(1), false)).toBe('The data source "shop" no longer exists.');
    expect(consoleBlocker(entry(1, { sql: null }), true)).toBe("This call ran no SQL.");
    expect(consoleBlocker(entry(1, { dataSourceId: null, dataSourceName: null }), false)).toContain("no data source");
  });
});
