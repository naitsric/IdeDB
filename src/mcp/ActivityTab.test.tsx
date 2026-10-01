// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataSource } from "../db/api";
import type { AuditEntry, AuditFilter, McpClient, McpEvent, ServerStatus } from "./api";

/** The store's audit log, as `mcp_audit_list` filters it. */
const server = {
  rows: [] as AuditEntry[],
  calls: [] as AuditFilter[],
  /** While set, each read waits for its own release, to answer them out of order. */
  held: null as (() => void)[] | null,
  status: { running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null } as ServerStatus,
  clients: [] as McpClient[],
};

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  mcpApi: {
    status: vi.fn(async () => server.status),
    settings: vi.fn(async () => ({
      enabled: server.status.running,
      port: 7412,
      maxRows: 200,
      statementTimeoutSecs: 30,
      writeTimeoutSecs: 600,
      approvalTimeoutSecs: 120,
    })),
    clients: vi.fn(async () => server.clients),
    neverWrite: vi.fn(async () => []),
    pendingApprovals: vi.fn(async () => []),
    audit: vi.fn(async (filter: AuditFilter) => {
      server.calls.push(filter);
      if (server.held) await new Promise<void>((release) => server.held!.push(release));
      const search = filter.search?.toLowerCase();
      return server.rows
        .filter(
          (e) =>
            (filter.clientId == null || e.clientId === filter.clientId) &&
            (filter.dataSourceId == null || e.dataSourceId === filter.dataSourceId) &&
            (filter.decision == null || e.decision === filter.decision) &&
            (!search || (e.sql ?? "").toLowerCase().includes(search)) &&
            (filter.beforeId == null || e.id < filter.beforeId),
        )
        .sort((a, b) => b.id - a.id)
        .slice(0, filter.limit ?? 100);
    }),
  },
}));

// `mcp://event`, delivered by hand.
let emit: (event: McpEvent) => void = () => {};
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, handler: (e: { payload: McpEvent }) => void) => {
    emit = (payload) => handler({ payload });
    return () => {};
  }),
}));

// Where "Open in Console" ends up; the console itself needs the whole workbench.
const newConsole = vi.fn();
vi.mock("../actions", () => ({ newConsole, editDataSource: vi.fn() }));

const { McpPanel } = await import("./McpPanel");
const { useMcp, initMcp } = await import("./store");
const { emptyLog, NO_FILTER } = await import("./activity");
const { useDataSources } = await import("../db/dataSources");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = Date.parse("2026-10-01T12:00:00.000Z");

const entry = (id: number, extra: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  at: new Date(NOW - (100 - id) * 1000).toISOString(),
  clientId: "claude-code",
  clientName: "claude-code",
  clientInfoName: "claude-code",
  clientInfoVersion: "2.1.0",
  protocolVersion: "2026-07-28",
  transport: "http",
  sessionKey: null,
  tool: "query",
  dataSourceId: "pg",
  dataSourceName: "shop",
  sql: `select * from orders where id = ${id}`,
  sqlTruncated: false,
  statementKind: "SELECT",
  reason: null,
  decision: "allowed",
  approvalWaitMs: null,
  elapsedMs: 34,
  rowCount: 12,
  truncated: false,
  error: null,
  ...extra,
});

const client = (id: string): McpClient => ({
  id,
  name: id,
  tokenPrefix: "idedb_abcdef",
  createdAt: "2026-09-01T12:00:00.000Z",
  lastSeenAt: null,
  lastClientName: null,
  lastClientVersion: null,
  revokedAt: null,
  grants: [],
});

const source = (id: string, name: string, color: string | null = null): DataSource => ({
  id,
  name,
  params: { engine: "postgres", host: "localhost", port: null, user: "u", database: "", sslMode: "prefer", path: "" },
  color,
  savePassword: true,
});

/** A read, a refused write that failed, and an approved write over the stdio bridge. */
const sample = () => [
  entry(1),
  entry(2, {
    tool: "execute",
    sql: "delete from orders",
    statementKind: "DELETE",
    decision: "denied",
    rowCount: null,
    elapsedMs: null,
    error: "This client may only read 'shop'.",
  }),
  entry(3, {
    clientId: "cursor",
    clientName: "cursor",
    clientInfoName: "Cursor",
    clientInfoVersion: "1.7",
    tool: "execute",
    transport: "bridge",
    sessionKey: "bridge-7f3a",
    dataSourceId: "lite",
    dataSourceName: "app.db",
    sql: "update users\nset active = false\nwhere id = 3",
    statementKind: "UPDATE",
    reason: "Deactivate the test user",
    decision: "approved",
    approvalWaitMs: 4200,
    rowCount: 1,
  }),
];

let root: Root;
let stop: () => void;

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

/** Starts the app's MCP state on the server as the test left it, then shows the tool window. */
async function render() {
  stop = initMcp();
  await settle();
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<McpPanel />));
  await settle();
}

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  server.rows = sample();
  server.calls = [];
  server.held = null;
  server.status = { running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null };
  server.clients = [client("claude-code"), client("cursor")];
  newConsole.mockClear();
  useDataSources.setState({
    loaded: true,
    sources: [source("pg", "shop", "#e5484d"), source("lite", "app.db")],
  });
  useMcp.setState({
    loaded: false,
    audit: [],
    tab: "activity",
    activity: { filter: NO_FILTER, log: emptyLog, selectedId: null, version: 0 },
  });
});

afterEach(async () => {
  stop();
  await act(async () => root.unmount());
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const text = () => document.body.textContent ?? "";
const byLabel = (label: string) => document.querySelector(`[aria-label="${label}"]`) as HTMLElement;
const rows = () => [...document.querySelectorAll<HTMLElement>('[role="option"][data-entry]')];
const rowIds = () => rows().map((r) => Number(r.dataset.entry));
const button = (label: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === label) as HTMLButtonElement;
const detail = () => byLabel("Activity details")?.textContent ?? "";

async function click(target: Element) {
  await act(async () => {
    target.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
  await settle();
}

async function press(target: Element, key: string, init: KeyboardEventInit = {}) {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  });
}

async function choose(select: HTMLElement, value: string) {
  await act(async () => {
    (select as HTMLSelectElement).value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settle();
}

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** Lets the search's typing pause pass. */
async function pause() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 250));
  });
  await settle();
}

describe("Activity tab", () => {
  it("is the tool window's first tab", async () => {
    await render();
    const tabs = [...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent);
    expect(tabs).toEqual(["Activity", "Clients", "Server"]);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Activity");
  });

  it("lists every call newest first: when, who, where, what and how it went", async () => {
    await render();
    expect(rowIds()).toEqual([3, 2, 1]);
    const [approved, denied, read] = rows();

    expect(approved.textContent).toContain("Approved");
    expect(approved.textContent).toContain("cursor");
    expect(approved.textContent).toContain("app.db");
    expect(approved.textContent).toContain("execute · UPDATE");
    // The SQL on one line.
    expect(approved.textContent).toContain("update users set active = false where id = 3");

    expect(denied.textContent).toContain("Denied");
    expect(denied.querySelector('[aria-label="Failed"]')?.getAttribute("title")).toBe("This client may only read 'shop'.");

    expect(read.textContent).toContain("Allowed");
    expect(read.textContent).toContain("query · SELECT");
    expect(read.textContent).toContain("select * from orders where id = 1");
    expect(read.textContent).toContain("12 rows · 34 ms");
    // A short time, the full one as its tooltip.
    const time = read.querySelector("time")!;
    expect(time.getAttribute("title")).toMatch(/Oct 1, 2026.*:\d\d\.000/);
    expect(read.querySelector('[aria-label="Failed"]')).toBeNull();
    expect(text()).toContain("3 entries");
  });

  it("shows new calls as they come", async () => {
    await render();
    await act(async () => emit({ kind: "audit", ...entry(4, { sql: "select 4" }) }));
    expect(rowIds()).toEqual([4, 3, 2, 1]);
  });

  it("filters history on the server and new calls here", async () => {
    await render();
    await choose(byLabel("Decision"), "denied");
    expect(server.calls.at(-1)).toMatchObject({ decision: "denied", beforeId: null, limit: 200 });
    expect(rowIds()).toEqual([2]);

    await act(async () => emit({ kind: "audit", ...entry(4) }));
    await act(async () => emit({ kind: "audit", ...entry(5, { decision: "denied" }) }));
    expect(rowIds()).toEqual([5, 2]);

    await choose(byLabel("Decision"), "");
    await choose(byLabel("Client"), "cursor");
    expect(server.calls.at(-1)).toMatchObject({ clientId: "cursor", decision: null });
    expect(rowIds()).toEqual([3]);
    await choose(byLabel("Client"), "");
    await choose(byLabel("Data source"), "pg");
    expect(rowIds()).toEqual([5, 4, 2, 1]);
  });

  it("drops a page asked for under filters since changed", async () => {
    await render();
    server.held = [];
    await choose(byLabel("Decision"), "denied");
    await choose(byLabel("Decision"), "approved");
    // The newer answer first, then the stale one.
    await act(async () => server.held![1]());
    await settle();
    await act(async () => server.held![0]());
    await settle();
    expect(rowIds()).toEqual([3]);
    expect(text()).toContain("1 entry");
  });

  it("searches the SQL as you type", async () => {
    await render();
    await type(byLabel("Search SQL") as HTMLInputElement, "  UPDATE users ");
    await pause();
    expect(server.calls.at(-1)).toMatchObject({ search: "UPDATE users" });
    expect(rowIds()).toEqual([3]);
    // Esc clears it.
    await press(byLabel("Search SQL"), "Escape");
    await settle();
    expect(rowIds()).toEqual([3, 2, 1]);
  });

  it("says when nothing matches, and clears the filters", async () => {
    await render();
    await type(byLabel("Search SQL") as HTMLInputElement, "truncate");
    await pause();
    expect(text()).toContain("No activity matches these filters");
    await click(button("Clear Filters"));
    expect(rowIds()).toEqual([3, 2, 1]);
    expect((byLabel("Search SQL") as HTMLInputElement).value).toBe("");
  });

  it("shows everything recorded about the selected call", async () => {
    await render();
    expect(detail()).toBe("");
    await click(rows()[0]);
    expect(rows()[0].getAttribute("aria-selected")).toBe("true");

    const shown = detail();
    expect(shown).toContain("UPDATE on app.db");
    expect(shown).toContain("Approved in IdeDB, then ran.");
    expect(shown).toContain("Verified token");
    expect(shown).toContain("Cursor 1.7 · unverified, as the client says");
    expect(shown).toContain("stdio bridge");
    expect(shown).toContain("bridge-7f3a");
    expect(shown).toContain("2026-07-28");
    expect(shown).toContain("“Deactivate the test user” · given by the client");
    expect(shown).toContain("Approval wait4.20 s");
    expect(shown).toContain("Elapsed34 ms");
    expect(shown).toContain("Rows1 row");
    expect(shown).toContain("SQL truncatedNo");
    expect(shown).toContain("Entry#3");
    // The whole statement, in the viewer.
    const sql = byLabel("SQL").textContent;
    expect(sql).toContain("update users");
    expect(sql).toContain("where id = 3");

    await click(rows()[1]);
    expect(detail()).toContain("This client may only read 'shop'.");
  });

  it("opens the SQL in a new console on its data source, without running it", async () => {
    await render();
    await click(rows()[0]);
    await click(button("Open in Console"));
    expect(newConsole).toHaveBeenCalledWith("lite", "update users\nset active = false\nwhere id = 3");
  });

  it("can't open SQL whose data source is gone", async () => {
    server.rows = [entry(1, { dataSourceId: "gone", dataSourceName: "legacy" })];
    await render();
    await click(rows()[0]);
    const open = button("Open in Console");
    expect(open.disabled).toBe(true);
    expect(open.parentElement?.getAttribute("title")).toBe('The data source "legacy" no longer exists.');
    expect(detail()).toContain("legacy· no longer exists");
    await press(byLabel("MCP activity"), "Enter");
    expect(newConsole).not.toHaveBeenCalled();
  });

  it("moves with the arrows, opens with Enter, and searches with ⌘F", async () => {
    await render();
    const list = byLabel("MCP activity");
    await press(list, "ArrowDown");
    await press(list, "ArrowDown");
    expect(useMcp.getState().activity.selectedId).toBe(2);
    expect(list.getAttribute("aria-activedescendant")).toBe("mcp-audit-2");
    await press(list, "ArrowUp");
    await press(list, "Enter");
    expect(newConsole).toHaveBeenCalledWith("lite", expect.stringContaining("update users"));

    await press(list, "f", { metaKey: true });
    expect(document.activeElement).toBe(byLabel("Search SQL"));
  });

  it("loads older entries page by page", async () => {
    server.rows = Array.from({ length: 700 }, (_, i) => entry(i + 1));
    await render();
    // The app keeps the newest 500; the newest page adds nothing to them.
    expect(server.calls.at(-1)).toMatchObject({ beforeId: null, limit: 200 });
    expect(text()).toContain("500+ entries");
    // Only the rows in view are rendered.
    expect(rows().length).toBeLessThan(40);

    // The next page starts below the oldest row shown.
    await click(button("Load Older"));
    expect(server.calls.at(-1)).toMatchObject({ beforeId: 201, limit: 200 });
    expect(text()).toContain("700+ entries");
    await click(button("Load Older"));
    expect(server.calls.at(-1)).toMatchObject({ beforeId: 1 });
    expect(text()).toContain("700 entries");
    expect(text()).toContain("Start of the log");
  });

  it("counts new calls above when scrolled down, instead of jumping", async () => {
    server.rows = Array.from({ length: 60 }, (_, i) => entry(i + 1));
    await render();
    const list = byLabel("MCP activity");
    await act(async () => {
      list.scrollTop = 440;
      list.dispatchEvent(new Event("scroll"));
    });
    await act(async () => emit({ kind: "audit", ...entry(61) }));
    await act(async () => emit({ kind: "audit", ...entry(62) }));
    // The rows being read stayed where they were.
    expect(list.scrollTop).toBe(440 + 2 * 44);
    expect(button("2 new")).toBeDefined();

    await click(button("2 new"));
    expect(list.scrollTop).toBe(0);
    expect(button("2 new")).toBeUndefined();
  });
});

describe("Activity empty states", () => {
  it("explains how to get activity", async () => {
    server.rows = [];
    server.clients = [];
    await render();
    expect(text()).toContain("No activity yet");
    expect(button("New Client")).toBeDefined();
    await click(button("Go to Clients"));
    expect(useMcp.getState().tab).toBe("clients");
  });

  it("says when the server is off", async () => {
    server.rows = [];
    server.status = { running: false, port: null, url: null, error: null };
    await render();
    expect(text()).toContain("The MCP server is off");
    expect(button("Turn On")).toBeDefined();
  });
});

describe("Show Activity", () => {
  it("shows what a client did, from the Clients tab", async () => {
    useMcp.setState({ tab: "clients", selectedClientId: "cursor" });
    await render();
    await click(byLabel("Show Activity"));
    expect(useMcp.getState().tab).toBe("activity");
    expect(server.calls.at(-1)).toMatchObject({ clientId: "cursor" });
    expect(rowIds()).toEqual([3]);
    expect((byLabel("Client") as HTMLSelectElement).value).toBe("cursor");
  });
});
