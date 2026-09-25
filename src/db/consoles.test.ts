// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryEvent } from "./api";

/** What the mocked backend answers per statement text. */
const replies = new Map<string, QueryEvent[]>();

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  api: {
    execute: vi.fn(async (_session: number, sql: string, onEvent: (e: QueryEvent) => void) => {
      for (const event of replies.get(sql) ?? [{ kind: "done", rowCount: 0, elapsedMs: 1, cancelled: false }]) {
        onEvent(event);
      }
    }),
    cancel: vi.fn(async () => {}),
    closeSession: vi.fn(async () => {}),
  },
}));

vi.mock("./dataSources", () => ({
  openSessionFor: vi.fn(async () => ({ id: 7, server: { engine: "postgres", version: "17", defaultSchema: "public" } })),
  useDataSources: { getState: () => ({ connect: vi.fn() }) },
}));

const { activeResult, rowGetter, useConsoles } = await import("./consoles");

const rows = (sql: string, values: number[]) =>
  replies.set(sql, [
    { kind: "columns", columns: [{ name: "n", typeName: "int4" }] },
    { kind: "rows", rows: values.map((v) => [v]) },
    { kind: "done", rowCount: values.length, elapsedMs: 3, cancelled: false },
  ]);

let consoleId = "";
const entry = () => useConsoles.getState().consoles[consoleId];
const titles = () => entry().results.map((r) => `${r.title}${r.pinned ? "*" : ""}`);

beforeEach(() => {
  replies.clear();
  consoleId = useConsoles.getState().create("ds-1");
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
    replies.set("selec 1", [{ kind: "error", message: "syntax error", position: 0 }]);
    const result = await useConsoles.getState().runStatement(consoleId, "selec 1");
    expect(result).toMatchObject({ status: "error", error: "syntax error", errorPosition: 0 });
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
