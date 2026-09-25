import type { DockviewApi } from "dockview-react";
import { create } from "zustand";

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

export function focusActiveConsole() {
  const { activeConsoleId } = useWorkbench.getState();
  if (activeConsoleId) dockview?.getPanel(consolePanelId(activeConsoleId))?.api.setActive();
}
