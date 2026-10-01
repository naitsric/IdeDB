import { describe, expect, it } from "vitest";
import type { ServerStatus } from "./api";
import { approvalsBadge, serverState, statusBarItem } from "./status";

const off: ServerStatus = { running: false, port: null, url: null, error: null };
const running: ServerStatus = { running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null };
const failed: ServerStatus = { ...off, error: "Port 7412 is already in use on 127.0.0.1." };

describe("serverState", () => {
  it("names each state", () => {
    expect(serverState(running, true)).toEqual({ tone: "success", label: "Running on 127.0.0.1:7412" });
    expect(serverState(failed, true)).toEqual({ tone: "danger", label: "Couldn't start" });
    expect(serverState(off, true)).toEqual({ tone: "warning", label: "Starting…" });
    expect(serverState(off, false)).toEqual({ tone: "idle", label: "Off" });
  });
});

describe("statusBarItem", () => {
  it("shows the port and the clients active", () => {
    expect(statusBarItem(running, true, 0)).toMatchObject({ tone: "success", text: "MCP :7412" });
    const two = statusBarItem(running, true, 2);
    expect(two.text).toBe("MCP :7412 · 2");
    expect(two.title).toContain("2 clients active in the last 2 minutes");
    expect(statusBarItem(running, true, 1).title).toContain("1 client active");
  });

  it("is muted when off and red with the reason when it failed", () => {
    expect(statusBarItem(off, false, 0)).toMatchObject({ tone: "idle", text: "MCP off" });
    const error = statusBarItem(failed, true, 0);
    expect(error).toMatchObject({ tone: "danger", text: "MCP error" });
    expect(error.title).toContain("already in use");
  });
});

describe("approvalsBadge", () => {
  it("counts approvals", () => {
    expect(approvalsBadge(1)).toBe("1 approval");
    expect(approvalsBadge(3)).toBe("3 approvals");
  });
});
