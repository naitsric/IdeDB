import { t } from "../i18n";
import type { ServerStatus } from "./api";

export type Tone = "success" | "warning" | "danger" | "idle";

/** The server's state in a few words, and the tone of its dot. */
export function serverState(status: ServerStatus, enabled: boolean): { tone: Tone; label: string } {
  if (status.running) return { tone: "success", label: t("mcp.status.running", { port: String(status.port) }) };
  if (status.error) return { tone: "danger", label: t("mcp.status.failed") };
  // Turned on, and the start not reported yet.
  if (enabled) return { tone: "warning", label: t("mcp.status.starting") };
  return { tone: "idle", label: t("mcp.status.off") };
}

/** Off because the user turned it off: not running, not starting, not failed. */
export function serverIsOff(status: ServerStatus, enabled: boolean): boolean {
  return !status.running && !status.error && !enabled;
}

/** The status bar's MCP item: its text, tooltip and dot. */
export function statusBarItem(
  status: ServerStatus,
  enabled: boolean,
  online: number,
): { tone: Tone; text: string; title: string } {
  const open = t("mcp.status.openHint");
  if (status.running) {
    const active =
      online > 0
        ? t(online === 1 ? "mcp.status.active.one" : "mcp.status.active.many", { count: online })
        : t("mcp.status.noneActive");
    return {
      tone: "success",
      text: online > 0 ? `MCP :${status.port} · ${online}` : `MCP :${status.port}`,
      title: t("mcp.status.title", { port: String(status.port), active, open }),
    };
  }
  if (status.error) return { tone: "danger", text: t("mcp.status.errorText"), title: t("mcp.status.errorTitle", { error: status.error }) };
  if (enabled) return { tone: "warning", text: t("mcp.status.startingText"), title: t("mcp.status.startingTitle", { open }) };
  return { tone: "idle", text: t("mcp.status.offText"), title: t("mcp.status.offTitle", { open }) };
}

/** The status bar's badge for writes waiting for approval. */
export function approvalsBadge(waiting: number): string {
  return t(waiting === 1 ? "mcp.approvals.badge.one" : "mcp.approvals.badge.many", { count: waiting });
}
