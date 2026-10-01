// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Endpoint } from "./api";

const endpoint = vi.fn<() => Promise<Endpoint>>();

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  mcpApi: { endpoint: () => endpoint() },
}));

// React's act() wants to know it runs in a test.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root;

/** The Claude Desktop configuration a fresh SnippetPicker shows, once the app answered. */
async function claudeDesktopConfig(token?: string) {
  // Fresh: the executable is asked once per app run.
  vi.resetModules();
  const { SnippetPicker } = await import("./SnippetPicker");
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<SnippetPicker port={7412} token={token} />));
  const block = document.querySelector('[aria-label="Claude Desktop configuration"]');
  return JSON.parse(block?.textContent ?? "").mcpServers.idedb;
}

beforeEach(() => {
  endpoint.mockReset();
});

afterEach(async () => {
  await act(async () => root.unmount());
  document.body.innerHTML = "";
});

describe("SnippetPicker", () => {
  it("has Claude Desktop run the executable the app runs from", async () => {
    const dev = "/Users/me/My Projects/idedb/target/debug/idedb";
    endpoint.mockResolvedValue({ url: "http://127.0.0.1:7412/mcp", bridgeCommand: dev });
    const server = await claudeDesktopConfig("idedb_shown-once");
    expect(server).toEqual({
      command: dev,
      args: ["mcp-bridge", "--port", "7412"],
      env: { IDEDB_MCP_TOKEN: "idedb_shown-once" },
    });
    expect(endpoint).toHaveBeenCalledTimes(1);
  });

  it("names the installed app when the app can't tell, or isn't asked", async () => {
    endpoint.mockResolvedValue({ url: "http://127.0.0.1:7412/mcp", bridgeCommand: null });
    expect((await claudeDesktopConfig()).command).toBe("/Applications/IdeDB.app/Contents/MacOS/idedb");
    await act(async () => root.unmount());

    endpoint.mockRejectedValue(new Error("no Tauri here"));
    const server = await claudeDesktopConfig();
    expect(server.command).toBe("/Applications/IdeDB.app/Contents/MacOS/idedb");
    expect(server.env).toEqual({ IDEDB_MCP_TOKEN: "<TOKEN>" });
  });
});
