import type { ServerStatus } from "./api";

export type Tone = "success" | "warning" | "danger" | "idle";

/** The server's state in a few words, and the tone of its dot. */
export function serverState(status: ServerStatus, enabled: boolean): { tone: Tone; label: string } {
  if (status.running) return { tone: "success", label: `Running on 127.0.0.1:${status.port}` };
  if (status.error) return { tone: "danger", label: "Couldn't start" };
  // Turned on, and the start not reported yet.
  if (enabled) return { tone: "warning", label: "Starting…" };
  return { tone: "idle", label: "Off" };
}

/** Off because the user turned it off: not running, not starting, not failed. */
export function serverIsOff(status: ServerStatus, enabled: boolean): boolean {
  return !status.running && !status.error && !enabled;
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The status bar's MCP item: its text, tooltip and dot. */
export function statusBarItem(
  status: ServerStatus,
  enabled: boolean,
  online: number,
): { tone: Tone; text: string; title: string } {
  const open = "Click to show or hide the MCP tool window.";
  if (status.running) {
    const active = online > 0 ? `${plural(online, "client", "clients")} active in the last 2 minutes` : "No client active";
    return {
      tone: "success",
      text: online > 0 ? `MCP :${status.port} · ${online}` : `MCP :${status.port}`,
      title: `MCP server on 127.0.0.1:${status.port}. ${active}. ${open}`,
    };
  }
  if (status.error) return { tone: "danger", text: "MCP error", title: `The MCP server couldn't start: ${status.error}` };
  if (enabled) return { tone: "warning", text: "MCP starting…", title: `The MCP server is starting. ${open}` };
  return { tone: "idle", text: "MCP off", title: `The MCP server is off. ${open}` };
}

/** The status bar's badge for writes waiting for approval. */
export function approvalsBadge(waiting: number): string {
  return plural(waiting, "approval", "approvals");
}
