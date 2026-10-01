import type { DockviewApi } from "dockview-react";
import { create } from "zustand";
import { confirmEndTransactions } from "../db/transactions";

/**
 * Lets code outside React (commands, actions) open and focus workbench
 * panels, and tracks which console is active.
 */

export const useWorkbench = create<{ activeConsoleId?: string }>(() => ({}));

let dockview: DockviewApi | null = null;

export function attachDockview(api: DockviewApi) {
  dockview = api;
}

export const consolePanelId = (consoleId: string) => `console:${consoleId}`;
export const WELCOME_PANEL_ID = "welcome";
export const EXPLORER_PANEL_ID = "explorer";
export const MCP_PANEL_ID = "mcp";

/** Opens a console tab next to the other consoles, or focuses it if already open. */
export function showConsole(consoleId: string) {
  if (!dockview) return;
  const id = consolePanelId(consoleId);
  const existing = dockview.getPanel(id);
  if (existing) {
    existing.api.setActive();
    return;
  }

  const sibling = dockview.panels.find((p) => p.id.startsWith("console:")) ?? dockview.getPanel(WELCOME_PANEL_ID);
  const explorer = dockview.getPanel(EXPLORER_PANEL_ID);
  dockview.addPanel({
    id,
    component: "console",
    title: "console",
    params: { consoleId },
    position: sibling
      ? { referencePanel: sibling, direction: "within" }
      : explorer
        ? { referencePanel: explorer, direction: "right" }
        : undefined,
  });
  dockview.getPanel(WELCOME_PANEL_ID)?.api.close();
}

/** Closes a console's tab, first asking to commit or roll back its open transaction. */
export async function closeConsole(consoleId: string) {
  if (!(await confirmEndTransactions([consoleId], "close the console"))) return;
  dockview?.getPanel(consolePanelId(consoleId))?.api.close();
}

/** Closes the focused tab (a console, the explorer or the welcome page), like ⌘W in an IDE. */
export async function closeActivePanel() {
  const panel = dockview?.activePanel;
  if (panel?.id.startsWith("console:")) await closeConsole(panel.id.slice("console:".length));
  else panel?.api.close();
}

export function hasActivePanel(): boolean {
  return !!dockview?.activePanel;
}

export function focusActiveConsole() {
  const { activeConsoleId } = useWorkbench.getState();
  if (activeConsoleId) dockview?.getPanel(consolePanelId(activeConsoleId))?.api.setActive();
}
