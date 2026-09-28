// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryEvent } from "./api";

/** What the mocked backend answers per statement text. */
const replies = new Map<string, QueryEvent[]>();
/** What successive fetches of more rows answer, in order. */
const fetchReplies: QueryEvent[][] = [];

const done = (rowCount: number, extra: Partial<Extract<QueryEvent, { kind: "done" }>> = {}): QueryEvent => ({
  kind: "done",
  rowCount,
  elapsedMs: 1,
  cancelled: false,
  hasMore: false,
  inTransaction: false,
  ...extra,
});

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  api: {
    execute: vi.fn(async (_session: number, sql: string, _firstRows: number | null, onEvent: (e: QueryEvent) => void) => {
      for (const event of replies.get(sql) ?? [done(0)]) {
        onEvent(event);
      }
    }),
    fetchMore: vi.fn(async (_session: number, _rows: number | null, onEvent: (e: QueryEvent) => void) => {
      for (const event of fetchReplies.shift() ?? []) {
        onEvent(event);
      }
    }),
    closeResult: vi.fn(async () => {}),
    cancel: vi.fn(async () => {}),
    closeSession: vi.fn(async () => {}),
  },
}));

/** The native "Discard N pending changes?" dialog. */
const ask = vi.fn(async () => true);
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask }));

const server = { engine: "postgres", version: "17", defaultSchema: "public" };
const openSessionFor = vi.fn(async (): Promise<{ id: number; server: typeof server } | null> => ({ id: 7, server }));
const sources = [{ id: "ds-1" }];
vi.mock("./dataSources", () => ({
  openSessionFor,
  useDataSources: { getState: () => ({ connect: vi.fn(), sources, explorers: {} }) },
}));

const { api } = await import("./api");
const { activeResult, rowGetter, setPendingChangesProbe, useConsoles } = await import("./consoles");

const rows = (sql: string, values: number[], hasMore = false) =>
  replies.set(sql, [
    { kind: "columns", columns: [{ name: "n", typeName: "int4" }] },
    { kind: "rows", rows: values.map((v) => [v]) },
    done(values.length, { hasMore }),
  ]);

let consoleId = "";
const entry = () => useConsoles.getState().consoles[consoleId];
const titles = () => entry().results.map((r) => `${r.title}${r.pinned ? "*" : ""}`);

beforeEach(() => {
  replies.clear();
  ask.mockClear();
  setPendingChangesProbe(() => 0);
  consoleId = useConsoles.getState().create("ds-1");
});

describe("opening the console's session", () => {
  it("opens one session for runs started while it connects, and runs only one of them", async () => {
    let resolve!: (opened: { id: number; server: typeof server }) => void;
    openSessionFor.mockClear();
    openSessionFor.mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    vi.mocked(api.execute).mockClear();
    // A real statement streams for a while; the second run must not replace it.
    vi.mocked(api.execute).mockImplementationOnce(async (_id, _sql, _firstRows, onEvent) => {
      await new Promise((r) => setTimeout(r, 0));
      onEvent(done(0));
    });
    const { runStatement } = useConsoles.getState();

    const first = runStatement(consoleId, "select 1");
    const second = runStatement(consoleId, "select 2");
    await Promise.resolve();
    resolve({ id: 9, server });
    const results = await Promise.all([first, second]);

    expect(openSessionFor).toHaveBeenCalledTimes(1);
    expect(api.execute).toHaveBeenCalledTimes(1);
    expect(results.filter(Boolean)).toHaveLength(1);
    expect(entry().sessionId).toBe(9);
  });

  it("closes a session that finishes opening after its console was closed", async () => {
    let resolve!: (opened: { id: number; server: typeof server }) => void;
    openSessionFor.mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    vi.mocked(api.closeSession).mockClear();
    const run = useConsoles.getState().runStatement(consoleId, "select 1");
    await Promise.resolve();
    await useConsoles.getState().remove(consoleId);
    resolve({ id: 11, server });
    expect(await run).toBeUndefined();
    expect(api.closeSession).toHaveBeenCalledWith(11);
  });
});

describe("pending data editor changes", () => {
  it("asks before a run replaces a result with unsubmitted edits, and keeps it when declined", async () => {
    rows("select 1", [1]);
    rows("select 2", [2]);
    const { runStatement } = useConsoles.getState();
    const first = await runStatement(consoleId, "select 1");
    setPendingChangesProbe((resultId) => (resultId === first!.id ? 3 : 0));

    ask.mockResolvedValueOnce(false);
    expect(await runStatement(consoleId, "select 2")).toBeUndefined();
    expect(ask).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ title: "Discard 3 pending changes?" }));
    expect(activeResult(entry())?.id).toBe(first!.id);
    expect(rowGetter(first!.id)(0)).toEqual([1]);

    ask.mockResolvedValueOnce(true);
    expect(await runStatement(consoleId, "select 2")).toMatchObject({ sql: "select 2" });
  });

  it("does not ask when the run opens a new tab or nothing is pending", async () => {
    const { runStatement } = useConsoles.getState();
    const first = await runStatement(consoleId, "select 1");
    await runStatement(consoleId, "select 1");
    setPendingChangesProbe((resultId) => (resultId === first!.id ? 1 : 0));
    await runStatement(consoleId, "select 2", { newTab: true });
    expect(ask).not.toHaveBeenCalled();
  });

  it("asks before closing a result tab with unsubmitted edits", async () => {
    const result = await useConsoles.getState().runStatement(consoleId, "select 1");
    setPendingChangesProbe(() => 1);
    ask.mockResolvedValueOnce(false);
    await useConsoles.getState().closeResult(consoleId, result!.id);
    expect(entry().results).toHaveLength(1);
  });
});

describe("result tabs", () => {
  it("reuses the active tab until it is pinned", async () => {
    rows("select 1", [1]);
    rows("select 2", [2]);
    const { runStatement, togglePin } = useConsoles.getState();

    await runStatement(consoleId, "select 1");
    await runStatement(consoleId, "select 2");
    expect(titles()).toEqual(["Result 1"]);
    expect(activeResult(entry())?.sql).toBe("select 2");

    togglePin(consoleId, entry().activeResultId!);
    await runStatement(consoleId, "select 1");
    expect(titles()).toEqual(["Result 1*", "Result 2"]);
    expect(activeResult(entry())?.title).toBe("Result 2");
  });

  it("opens a new tab on request and streams rows into it", async () => {
    rows("select 1", [1]);
    rows("select 2", [2, 3]);
    const { runStatement } = useConsoles.getState();

    await runStatement(consoleId, "select 1");
    const second = await runStatement(consoleId, "select 2", { newTab: true });
    expect(titles()).toEqual(["Result 1", "Result 2"]);
    expect(second).toMatchObject({ status: "done", rowCount: 2 });
    expect(rowGetter(second!.id)(1)).toEqual([3]);
  });

  it("reports errors with their position", async () => {
    replies.set("selec 1", [{ kind: "error", message: "syntax error", position: 0, inTransaction: false }]);
    const result = await useConsoles.getState().runStatement(consoleId, "selec 1");
    expect(result).toMatchObject({ status: "error", error: "syntax error", errorPosition: 0 });
  });

  it("tracks whether the session has a transaction open", async () => {
    replies.set("begin", [done(0, { inTransaction: true })]);
    replies.set("select 1/0", [{ kind: "error", message: "division by zero", position: null, inTransaction: true }]);
    const { runStatement } = useConsoles.getState();
    await runStatement(consoleId, "begin");
    expect(entry().inTransaction).toBe(true);
    await runStatement(consoleId, "select 1/0");
    expect(entry().inTransaction).toBe(true);
    await runStatement(consoleId, "rollback");
    expect(entry().inTransaction).toBe(false);
  });

  it("closing a tab frees its rows and activates a neighbour", async () => {
    rows("select 1", [1]);
    rows("select 2", [2]);
    rows("select 3", [3]);
    const { runStatement, closeResult } = useConsoles.getState();
    const first = await runStatement(consoleId, "select 1");
    const second = await runStatement(consoleId, "select 2", { newTab: true });
    await runStatement(consoleId, "select 3", { newTab: true });

    await closeResult(consoleId, second!.id);
    expect(titles()).toEqual(["Result 1", "Result 3"]);
    expect(rowGetter(second!.id)(0)).toBeUndefined();

    useConsoles.getState().selectResult(consoleId, first!.id);
    await closeResult(consoleId, first!.id);
    expect(activeResult(entry())?.title).toBe("Result 3");
  });

  it("numbers new tabs after the last one opened, not the count left", async () => {
    const { runStatement, closeResult } = useConsoles.getState();
    const first = await runStatement(consoleId, "select 1");
    await closeResult(consoleId, first!.id);
    await runStatement(consoleId, "select 1");
    expect(titles()).toEqual(["Result 2"]);
  });

  it("frees every result when the console is removed", async () => {
    rows("select 1", [1]);
    const result = await useConsoles.getState().runStatement(consoleId, "select 1");
    await useConsoles.getState().remove(consoleId);
    expect(rowGetter(result!.id)(0)).toBeUndefined();
  });
});

describe("fetching on demand", () => {
  beforeEach(() => {
    fetchReplies.length = 0;
  });

  it("asks for the first page only and remembers that more rows are open", async () => {
    rows("select big", [1, 2], true);
    vi.mocked(api.execute).mockClear();
    const result = await useConsoles.getState().runStatement(consoleId, "select big");
    expect(api.execute).toHaveBeenCalledWith(expect.any(Number), "select big", 500, expect.any(Function));
    expect(result).toMatchObject({ rowCount: 2, more: "open" });
  });

  it("appends fetched rows in place until the rest runs out", async () => {
    rows("select big", [1, 2], true);
    const result = await useConsoles.getState().runStatement(consoleId, "select big");
    fetchReplies.push([{ kind: "rows", rows: [[3], [4]] }, done(2, { hasMore: true })]);
    fetchReplies.push([{ kind: "rows", rows: [[5]] }, done(1)]);

    await useConsoles.getState().fetchMore(consoleId, result!.id);
    expect(activeResult(entry())).toMatchObject({ rowCount: 4, more: "open", fetching: undefined });
    await useConsoles.getState().fetchMore(consoleId, result!.id, true);
    expect(api.fetchMore).toHaveBeenLastCalledWith(expect.any(Number), null, expect.any(Function));
    expect(activeResult(entry())).toMatchObject({ rowCount: 5, more: "none" });
    expect([0, 1, 2, 3, 4].map((i) => rowGetter(result!.id)(i))).toEqual([[1], [2], [3], [4], [5]]);
  });

  it("does not fetch when nothing is open", async () => {
    rows("select 1", [1]);
    const result = await useConsoles.getState().runStatement(consoleId, "select 1");
    vi.mocked(api.fetchMore).mockClear();
    await useConsoles.getState().fetchMore(consoleId, result!.id);
    expect(api.fetchMore).not.toHaveBeenCalled();
  });

  it("marks the rest released when the session runs something else", async () => {
    rows("select big", [1, 2], true);
    const { runStatement } = useConsoles.getState();
    const first = await runStatement(consoleId, "select big");
    useConsoles.getState().togglePin(consoleId, first!.id);
    await runStatement(consoleId, "select 1");
    expect(entry().results.find((r) => r.id === first!.id)).toMatchObject({ more: "closed" });
  });

  it("shows why a fetch failed and stops offering more", async () => {
    rows("select big", [1, 2], true);
    const result = await useConsoles.getState().runStatement(consoleId, "select big");
    fetchReplies.push([{ kind: "error", message: "The result is no longer open", position: null, inTransaction: false }]);
    await useConsoles.getState().fetchMore(consoleId, result!.id);
    expect(activeResult(entry())).toMatchObject({ more: "closed", fetchError: "The result is no longer open", rowCount: 2 });
  });

  it("releases the rest on request and when its tab closes", async () => {
    rows("select big", [1, 2], true);
    const { runStatement, closeCursor, closeResult } = useConsoles.getState();
    await runStatement(consoleId, "select big");
    vi.mocked(api.closeResult).mockClear();
    await closeCursor(consoleId);
    expect(api.closeResult).toHaveBeenCalledTimes(1);
    expect(activeResult(entry())?.more).toBe("closed");

    const reopened = await runStatement(consoleId, "select big");
    await closeResult(consoleId, reopened!.id);
    expect(api.closeResult).toHaveBeenCalledTimes(2);
  });
});
