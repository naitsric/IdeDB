// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataSource } from "../db/api";
import type { Grant, McpClient, McpSettings } from "./api";

const calls: { setGrants: [string, Grant[]][]; setNeverWrite: [string, boolean][]; saveSettings: McpSettings[] } = {
  setGrants: [],
  setNeverWrite: [],
  saveSettings: [],
};

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  mcpApi: {
    clients: vi.fn(async () => (await import("./store")).useMcp.getState().clients),
    neverWrite: vi.fn(async () => (await import("./store")).useMcp.getState().neverWrite),
    setGrants: vi.fn(async (clientId: string, grants: Grant[]) => {
      calls.setGrants.push([clientId, grants]);
      const client = (await import("./store")).useMcp.getState().clients.find((c) => c.id === clientId)!;
      return { ...client, grants };
    }),
    setNeverWrite: vi.fn(async (id: string, value: boolean) => {
      calls.setNeverWrite.push([id, value]);
    }),
    createClient: vi.fn(async (name: string) => ({
      client: client(name),
      token: "idedb_TOKEN-shown-once_0123456789abcdefghijklmnopq",
    })),
    saveSettings: vi.fn(async (settings: McpSettings) => {
      calls.saveSettings.push(settings);
      return { running: true, port: settings.port, url: `http://127.0.0.1:${settings.port}/mcp`, error: null };
    }),
  },
}));

// The dock itself needs Tauri's window; only whether the tool window is asked for matters here.
const showMcp = vi.fn();
vi.mock("../workbench/Workbench", () => ({ showMcp, toggleMcp: vi.fn() }));

const { McpPanel } = await import("./McpPanel");
const { McpDialogs } = await import("./McpDialogs");
const { useMcp, openNewClient } = await import("./store");
const { useDataSources } = await import("../db/dataSources");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = Date.parse("2026-10-01T12:00:00.000Z");

const settings: McpSettings = {
  enabled: true,
  port: 7412,
  maxRows: 200,
  statementTimeoutSecs: 30,
  writeTimeoutSecs: 600,
  approvalTimeoutSecs: 120,
};

const source = (id: string, name: string, extra: Partial<DataSource["params"]> = {}, savePassword = true): DataSource => ({
  id,
  name,
  params: { engine: "postgres", host: "localhost", port: null, user: "u", database: "", sslMode: "prefer", path: "", ...extra },
  color: null,
  savePassword,
});

const client = (id: string, extra: Partial<McpClient> = {}): McpClient => ({
  id,
  name: id,
  tokenPrefix: `idedb_${id.slice(0, 6).padEnd(6, "x")}`,
  createdAt: "2026-09-01T12:00:00.000Z",
  lastSeenAt: null,
  lastClientName: null,
  lastClientVersion: null,
  revokedAt: null,
  grants: [],
  ...extra,
});

let root: Root;

async function render(ui = <McpPanel />) {
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(ui));
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  calls.setGrants = [];
  calls.setNeverWrite = [];
  calls.saveSettings = [];
  useDataSources.setState({
    loaded: true,
    sources: [
      source("pg", "shop"),
      source("my", "crm", { engine: "mysql" }, false),
      source("lite", "app.db", { engine: "sqlite", path: "/tmp/app.db" }, false),
    ],
  });
  useMcp.setState({
    loaded: true,
    loadError: null,
    tab: "clients",
    selectedClientId: null,
    settings,
    status: { running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null },
    neverWrite: [],
    clients: [
      client("claude-code", {
        lastSeenAt: new Date(NOW - 30_000).toISOString(),
        lastClientName: "claude-code",
        lastClientVersion: "2.1.0",
        grants: [{ dataSourceId: "pg", access: "read" }],
      }),
      client("old", { revokedAt: "2026-09-20T12:00:00.000Z" }),
    ],
  });
});

afterEach(async () => {
  await act(async () => root.unmount());
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const text = () => document.body.textContent ?? "";
const byLabel = (label: string) => document.querySelector(`[aria-label="${label}"]`) as HTMLElement;
const radio = (group: string, label: string) =>
  [...byLabel(group).querySelectorAll('[role="radio"]')].find((r) => r.textContent === label) as HTMLElement;

async function click(target: Element) {
  await act(async () => {
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

describe("Clients tab", () => {
  it("lists clients with their token prefix, presence, clientInfo and revoked state", async () => {
    await render();
    const list = byLabel("MCP clients").textContent ?? "";
    expect(list).toContain("claude-code");
    expect(list).toContain("idedb_claude…");
    expect(list).toContain("Online");
    expect(list).toContain("claude-code 2.1.0");
    expect(list).toContain("Revoked");
    expect(text()).toContain("Reported as claude-code 2.1.0");
  });

  it("shows each data source's access, and why some can't be served", async () => {
    await render();
    expect(radio("Access to shop", "Read").getAttribute("aria-checked")).toBe("true");
    expect(text()).toContain("Its password isn't saved");
    // Granting is blocked where the password isn't saved; SQLite files are fine.
    expect(radio("Access to crm", "Read").getAttribute("aria-disabled")).toBe("true");
    expect(radio("Access to app.db", "Write").getAttribute("aria-disabled")).toBeNull();
  });

  it("saves a new access level and the never-write lock", async () => {
    await render();
    await click(radio("Access to app.db", "Write"));
    expect(calls.setGrants).toEqual([
      [
        "claude-code",
        [
          { dataSourceId: "pg", access: "read" },
          { dataSourceId: "lite", access: "write" },
        ],
      ],
    ]);
    await click(byLabel("Never write on shop"));
    expect(calls.setNeverWrite).toEqual([["pg", true]]);
    expect(byLabel("Never write on shop").getAttribute("aria-pressed")).toBe("true");
    // Never write blocks granting a new write.
    expect(radio("Access to shop", "Write").getAttribute("aria-disabled")).toBe("true");
  });

  it("changes nothing for a revoked client", async () => {
    useMcp.setState({ selectedClientId: "old" });
    await render();
    expect(text()).toContain("its token no longer works");
    expect(radio("Access to shop", "Read").getAttribute("aria-disabled")).toBe("true");
  });

  it("teaches what to do when there are no clients, and when the server is off", async () => {
    useMcp.setState({
      clients: [],
      settings: { ...settings, enabled: false },
      status: { running: false, port: null, url: null, error: null },
    });
    await render();
    expect(text()).toContain("No MCP clients yet");
    expect(text()).toContain("The MCP server is off");
  });

  it("offers to add a data source when there is none", async () => {
    useDataSources.setState({ sources: [] });
    await render();
    expect(text()).toContain("No data sources yet");
  });
});

describe("New client", () => {
  it("asks for a name, then shows the token once with ready-to-paste snippets", async () => {
    await render(<McpDialogs />);
    await act(async () => openNewClient());
    expect(text()).toContain("New MCP Client");
    // Names already taken are not suggested.
    expect(text()).not.toContain("claude-code");
    expect(text()).toContain("cursor");
    await type(byLabel("Client name") as HTMLInputElement, "  cursor ");
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Create")!);

    expect(text()).toContain("Token for cursor");
    expect(text()).toContain("can't show it again");
    const token = "idedb_TOKEN-shown-once_0123456789abcdefghijklmnopq";
    expect(document.querySelector("code")?.textContent).toBe(token);
    expect(text()).toContain(
      `claude mcp add --transport http idedb http://127.0.0.1:7412/mcp --header "Authorization: Bearer ${token}"`,
    );
    expect(useMcp.getState().selectedClientId).toBe("cursor");

    // Done opens the tool window, where its access is granted.
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Done")!);
    expect(useMcp.getState().dialog).toBeNull();
    expect(showMcp).toHaveBeenCalledWith("clients");
  });
});

describe("Server tab", () => {
  it("validates and saves the port and limits", async () => {
    useMcp.setState({ tab: "server" });
    await render();
    expect(text()).toContain("Running on 127.0.0.1:7412");
    expect(text()).toContain("read-only");
    const save = [...document.querySelectorAll("button")].find((b) => b.textContent === "Save") as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    await type(byLabel("Port") as HTMLInputElement, "70000");
    expect(text()).toContain("A port from 1 to 65535.");
    expect(save.disabled).toBe(true);

    await type(byLabel("Port") as HTMLInputElement, "7500");
    expect(text()).toContain("The server restarts on the new port.");
    expect(save.disabled).toBe(false);
    await click(save);
    expect(calls.saveSettings).toEqual([{ ...settings, port: 7500 }]);
    expect(useMcp.getState().status.port).toBe(7500);
  });
});
