// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryEvent } from "./api";

/**
 * A fake server session: tracks whether a transaction block is open the way
 * the drivers report it, and lets a test script failures per statement.
 */
const server = { inTransaction: false, failures: new Map<string, string>(), lost: new Set<string>() };
const executed: string[] = [];

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  api: {
    execute: vi.fn(async (_session: number, sql: string, _firstRows: number | null, onEvent: (e: QueryEvent) => void) => {
      executed.push(sql);
      if (server.lost.has(sql)) {
        server.inTransaction = false;
        onEvent({
          kind: "error",
          message: "The connection to the server was lost and has been re-established. The transaction that was open was rolled back by the server.",
          position: null,
          inTransaction: false,
        });
        return;
      }
      const failure = server.failures.get(sql);
      if (failure) {
        onEvent({ kind: "error", message: failure, position: null, inTransaction: server.inTransaction });
        return;
      }
      if (/^(begin|start transaction)$/i.test(sql)) server.inTransaction = true;
      else if (/^(commit|rollback)$/i.test(sql)) server.inTransaction = false;
      else if (/^create /i.test(sql)) server.inTransaction = false; // implicit commit, as MySQL does for DDL
      if (/^select /i.test(sql)) {
        // A read larger than a page: the rest stays open on the session.
        onEvent({ kind: "columns", columns: [{ name: "a", typeName: "int4" }] });
        onEvent({ kind: "rows", rows: [[1], [2]] });
        onEvent({ kind: "done", rowCount: 2, elapsedMs: 1, cancelled: false, hasMore: true, inTransaction: server.inTransaction });
        return;
      }
      onEvent({ kind: "done", rowCount: 0, elapsedMs: 1, cancelled: false, hasMore: false, inTransaction: server.inTransaction });
    }),
    fetchMore: vi.fn(async (_session: number, _rows: number | null, onEvent: (e: QueryEvent) => void) => {
      executed.push("<fetch more>");
      onEvent({ kind: "rows", rows: [[3]] });
      onEvent({ kind: "done", rowCount: 1, elapsedMs: 1, cancelled: false, hasMore: true, inTransaction: server.inTransaction });
    }),
    closeResult: vi.fn(async () => {}),
    cancel: vi.fn(async () => {}),
    closeSession: vi.fn(async () => {}),
  },
}));

const ask = vi.fn(async () => true);
const message = vi.fn(async (): Promise<string> => "Cancel");
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask, message }));
const invoke = vi.fn(async () => {});
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  invoke,
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onCloseRequested: async () => () => {} }) }));

const sources = [
  { id: "pg", name: "local pg", params: { engine: "postgres" } },
  { id: "my", name: "local mysql", params: { engine: "mysql" } },
];
vi.mock("./dataSources", () => ({
  openSessionFor: vi.fn(async () => ({ id: 7, server: { engine: "postgres", version: "17", defaultSchema: "public" } })),
  useDataSources: { getState: () => ({ connect: vi.fn(), sources, explorers: {} }) },
}));

const { useConsoles } = await import("./consoles");
const tx = await import("./transactions");
const {
  beginSql,
  confirmEndTransactions,
  consolesWithOpenTransactions,
  endTransaction,
  formatElapsed,
  isTransactionControl,
  modeOf,
  needsBegin,
  nextTransactionState,
  quitApp,
  setMode,
  useTransactions,
} = tx;

describe("rules", () => {
  it("opens transactions with each engine's syntax", () => {
    expect(beginSql("postgres")).toBe("begin");
    expect(beginSql("sqlite")).toBe("begin");
    expect(beginSql("mysql")).toBe("start transaction");
  });

  it("recognizes transaction control, past leading comments", () => {
    for (const sql of ["BEGIN", "start  transaction", "commit", "END", "rollback to savepoint a", "savepoint a", "release a", "abort"]) {
      expect(isTransactionControl(sql), sql).toBe(true);
    }
    expect(isTransactionControl("-- undo it\n/* all */ rollback")).toBe(true);
    for (const sql of ["select 1", "update t set committed = true", "beginning", "with x as (select 1) select * from x"]) {
      expect(isTransactionControl(sql), sql).toBe(false);
    }
  });

  it("begins only for the user's SQL in manual mode with no transaction open", () => {
    expect(needsBegin("manual", false, "update t set a = 1", false)).toBe(true);
    expect(needsBegin("auto", false, "update t set a = 1", false)).toBe(false);
    expect(needsBegin("manual", true, "update t set a = 1", false)).toBe(false);
    expect(needsBegin("manual", false, "commit", false)).toBe(false);
    expect(needsBegin("manual", false, "select * from t", true)).toBe(false);
  });

  it("counts statements, not transaction control, and marks failed PostgreSQL transactions", () => {
    const base = { wasOpen: true, nowOpen: true, sessionLost: false, engine: "postgres" as const, at: 5 };
    let open = nextTransactionState(undefined, { ...base, wasOpen: false, sql: "begin" }).open;
    expect(open).toEqual({ since: 5, statements: 0, failed: false });
    open = nextTransactionState(open, { ...base, sql: "update t set a = 1" }).open;
    expect(open).toMatchObject({ statements: 1, failed: false });
    open = nextTransactionState(open, { ...base, sql: "update t set a = 'x'", error: "invalid input syntax" }).open;
    expect(open).toMatchObject({ statements: 2, failed: true });
    // ROLLBACK TO SAVEPOINT recovers it.
    open = nextTransactionState(open, { ...base, sql: "rollback to savepoint s" }).open;
    expect(open).toMatchObject({ statements: 2, failed: false });
    // Errors do not abort a MySQL transaction.
    expect(nextTransactionState(open, { ...base, engine: "mysql", sql: "x", error: "boom" }).open?.failed).toBe(false);
  });

  it("explains a transaction that ended without the user ending it", () => {
    const ended = { wasOpen: true, nowOpen: false, sessionLost: false, at: 0 };
    expect(nextTransactionState(undefined, { ...ended, sql: "commit" }).notice).toBeUndefined();
    expect(nextTransactionState(undefined, { ...ended, sql: "rollback" }).notice).toBeUndefined();
    expect(nextTransactionState(undefined, { ...ended, sql: "create table t (a int)" }).notice).toMatch(/committed it implicitly/);
    expect(nextTransactionState(undefined, { ...ended, sql: "select 1", error: "lost\n  rolled back" }).notice).toBe(
      "The open transaction ended: lost rolled back",
    );
    expect(nextTransactionState(undefined, { ...ended, sql: "select 1", sessionLost: true }).notice).toMatch(/session was lost/);
    expect(nextTransactionState(undefined, { ...ended, wasOpen: false, sql: "select 1" }).notice).toBeUndefined();
  });

  it("formats the open time as a clock", () => {
    expect(formatElapsed(7_400)).toBe("0:07");
    expect(formatElapsed(760_000)).toBe("12:40");
    expect(formatElapsed(3_729_000)).toBe("1:02:09");
    expect(formatElapsed(-5)).toBe("0:00");
  });
});

let consoleId = "";
const entry = () => useConsoles.getState().consoles[consoleId];
const run = (sql: string, options?: Parameters<ReturnType<typeof useConsoles.getState>["runStatement"]>[2]) =>
  useConsoles.getState().runStatement(consoleId, sql, options);

beforeEach(() => {
  server.inTransaction = false;
  server.failures.clear();
  server.lost.clear();
  executed.length = 0;
  message.mockReset();
  message.mockResolvedValue("Cancel");
  invoke.mockClear();
  consoleId = useConsoles.getState().create("pg");
});

describe("manual mode", () => {
  it("opens a transaction before the first statement and keeps later ones in it", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    await run("delete from t");
    expect(executed).toEqual(["begin", "update t set a = 1", "delete from t"]);
    expect(entry().inTransaction).toBe(true);
    expect(useTransactions.getState().open[consoleId]).toMatchObject({ statements: 2, failed: false });
  });

  it("uses START TRANSACTION on MySQL", async () => {
    consoleId = useConsoles.getState().create("my");
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    expect(executed[0]).toBe("start transaction");
  });

  it("leaves Auto mode, table loads and transaction control alone", async () => {
    await run("update t set a = 1");
    setMode(consoleId, "manual");
    await run("select * from t", { table: { schema: "public", name: "t" } });
    await run("commit");
    expect(executed).toEqual(["update t set a = 1", "select * from t", "commit"]);
  });

  it("does not run a statement when the transaction cannot be opened", async () => {
    setMode(consoleId, "manual");
    server.failures.set("begin", "permission denied");
    const result = await run("delete from t");
    expect(executed).toEqual(["begin"]);
    expect(result?.status).toBe("error");
    expect(result?.error).toMatch(/Could not open a transaction \(manual mode\): permission denied/);
  });

  it("persists the mode per console", () => {
    setMode(consoleId, "manual");
    expect(JSON.parse(localStorage.getItem("idedb.transactionModes.v1") ?? "{}")[consoleId]).toBe("manual");
    setMode(consoleId, "auto");
    expect(modeOf(consoleId)).toBe("auto");
    expect(JSON.parse(localStorage.getItem("idedb.transactionModes.v1") ?? "{}")[consoleId]).toBeUndefined();
  });
});

describe("commit and rollback", () => {
  it("commits the open transaction", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    expect(await endTransaction(consoleId, "commit")).toBe(true);
    expect(executed.at(-1)).toBe("commit");
    expect(entry().inTransaction).toBe(false);
    expect(useTransactions.getState().open[consoleId]).toBeUndefined();
  });

  it("reloads the data editor's table after a rollback", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    await run("select * from t", { table: { schema: "public", name: "t" } });
    expect(await endTransaction(consoleId, "rollback")).toBe(true);
    await vi.waitFor(() => expect(executed.slice(-2)).toEqual(["rollback", "select * from t"]));
    // A reload is a table load: it does not open a new transaction in manual mode.
    expect(entry().inTransaction).toBe(false);
  });

  it("says when PostgreSQL rolled back a failed transaction instead of committing", async () => {
    setMode(consoleId, "manual");
    server.failures.set("update t set a = 'x'", "invalid input syntax");
    await run("update t set a = 'x'");
    expect(useTransactions.getState().open[consoleId]?.failed).toBe(true);
    await endTransaction(consoleId, "commit");
    expect(useTransactions.getState().notices[consoleId]).toMatch(/rolled the transaction back instead of committing/);
  });

  it("keeps the transaction and says why when commit fails", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    server.failures.set("commit", "deferred constraint violated");
    expect(await endTransaction(consoleId, "commit")).toBe(false);
    expect(entry().inTransaction).toBe(true);
    expect(useTransactions.getState().notices[consoleId]).toBe("Commit failed: deferred constraint violated");
  });
});

describe("transactions that end on their own", () => {
  it("resets the indicator and tells the user when the connection was lost", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    server.lost.add("select 1");
    await run("select 1");
    expect(entry().inTransaction).toBe(false);
    expect(useTransactions.getState().open[consoleId]).toBeUndefined();
    expect(useTransactions.getState().notices[consoleId]).toMatch(/rolled back by the server/);
  });

  it("tells the user about an implicit commit", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    await run("create table u (a int)");
    expect(useTransactions.getState().notices[consoleId]).toMatch(/committed it implicitly/);
  });
});

describe("guard rails", () => {
  it("lists consoles with open transactions, per data source", async () => {
    const other = useConsoles.getState().create("my");
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    const { consoles } = useConsoles.getState();
    expect(consolesWithOpenTransactions(consoles)).toContain(consoleId);
    expect(consolesWithOpenTransactions(consoles, "my")).not.toContain(consoleId);
    expect(consolesWithOpenTransactions(consoles)).not.toContain(other);
  });

  it("goes on without asking when nothing is open", async () => {
    expect(await confirmEndTransactions([consoleId], "close the console")).toBe(true);
    expect(message).not.toHaveBeenCalled();
  });

  it("stops on Cancel, leaving the transaction open", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    expect(await confirmEndTransactions([consoleId], "close the console")).toBe(false);
    expect(message).toHaveBeenCalledWith(
      expect.stringContaining("close the console"),
      expect.objectContaining({ buttons: { yes: "Commit", no: "Rollback", cancel: "Cancel" } }),
    );
    expect(entry().inTransaction).toBe(true);
    expect(executed.at(-1)).toBe("update t set a = 1");
  });

  it("commits or rolls back as chosen, then goes on", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    message.mockResolvedValueOnce("Rollback");
    expect(await confirmEndTransactions([consoleId], "disconnect")).toBe(true);
    expect(executed.at(-1)).toBe("rollback");

    await run("update t set a = 2");
    message.mockResolvedValueOnce("Commit");
    expect(await confirmEndTransactions([consoleId], "disconnect")).toBe(true);
    expect(executed.at(-1)).toBe("commit");
  });

  it("quits only after the open transactions are dealt with", async () => {
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    await quitApp();
    expect(invoke).not.toHaveBeenCalled();
    message.mockResolvedValueOnce("Commit");
    await quitApp();
    expect(invoke).toHaveBeenCalledWith("app_quit");
  });
});

describe("with results read a page at a time", () => {
  const firstResult = () => entry().results[0];

  it("releases a result's open rest before Commit or Rollback", async () => {
    setMode(consoleId, "manual");
    await run("select * from t");
    expect(executed).toEqual(["begin", "select * from t"]);
    expect(firstResult().more).toBe("open");

    expect(await endTransaction(consoleId, "commit")).toBe(true);
    // The session closed the rest before COMMIT; the grid must stop offering it.
    expect(firstResult().more).toBe("closed");
    await useConsoles.getState().fetchMore(consoleId, firstResult().id);
    expect(executed).toEqual(["begin", "select * from t", "commit"]);
  });

  it("releases the previous result before a manual transaction opens", async () => {
    await run("select * from t");
    useConsoles.getState().togglePin(consoleId, firstResult().id);
    setMode(consoleId, "manual");
    await run("update t set a = 1");
    expect(executed).toEqual(["select * from t", "begin", "update t set a = 1"]);
    expect(firstResult().more).toBe("closed");
  });

  it("keeps the transaction open and uncounted while fetching more", async () => {
    setMode(consoleId, "manual");
    await run("select * from t");
    await useConsoles.getState().fetchMore(consoleId, firstResult().id);
    expect(executed).toEqual(["begin", "select * from t", "<fetch more>"]);
    expect(firstResult()).toMatchObject({ rowCount: 3, more: "open" });
    expect(entry().inTransaction).toBe(true);
    expect(useTransactions.getState().open[consoleId]).toMatchObject({ statements: 1, failed: false });
  });
});
