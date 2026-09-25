import "dockview-react/dist/styles/dockview.css";
import {
  DockviewReact,
  type DockviewApi,
  type DockviewReadyEvent,
  type DockviewTheme,
  type IDockviewPanelProps,
} from "dockview-react";
import type { FunctionComponent } from "react";
import { useConsoles } from "../db/consoles";
import {
  attachDockview,
  EXPLORER_PANEL_ID,
  showConsole,
  useWorkbench,
  WELCOME_PANEL_ID,
} from "./bridge";
import { ConsolePanel } from "./panels/ConsolePanel";
import { ExplorerPanel } from "./panels/ExplorerPanel";
import { WelcomePanel } from "./panels/WelcomePanel";

const LAYOUT_STORAGE_KEY = "idedb.layout.v2";

const theme: DockviewTheme = {
  name: "idedb",
  className: "dockview-theme-idedb",
  dndOverlayMounting: "absolute",
  dndTabIndicator: "line",
};

const components: Record<string, FunctionComponent<IDockviewPanelProps<any>>> = {
  explorer: ExplorerPanel,
  console: ConsolePanel,
  welcome: WelcomePanel,
};

let api: DockviewApi | null = null;

function consolePanels(dockview: DockviewApi) {
  return dockview.panels.filter((p) => p.id.startsWith("console:"));
}

/** The editor area: the console group, or the welcome panel when there are no consoles. */
function editorAnchor(dockview: DockviewApi) {
  return consolePanels(dockview)[0] ?? dockview.getPanel(WELCOME_PANEL_ID);
}

function addWelcome(dockview: DockviewApi) {
  const explorer = dockview.getPanel(EXPLORER_PANEL_ID);
  dockview.addPanel({
    id: WELCOME_PANEL_ID,
    component: "welcome",
    title: "Welcome",
    position: explorer ? { referencePanel: explorer, direction: "right" } : undefined,
  });
}

export function addExplorer(dockview: DockviewApi) {
  const anchor = editorAnchor(dockview);
  return dockview.addPanel({
    id: EXPLORER_PANEL_ID,
    component: "explorer",
    title: "Database Explorer",
    position: anchor ? { referencePanel: anchor, direction: "left" } : undefined,
    initialWidth: 300,
  });
}

function defaultLayout(dockview: DockviewApi) {
  addWelcome(dockview);
  addExplorer(dockview);
  const consoles = Object.keys(useConsoles.getState().consoles);
  for (const id of consoles) showConsole(id);
}

/** Like IntelliJ's ⌘1: show and focus a tool window, or hide it if it already has focus. */
export function toggleExplorer() {
  if (!api) return;
  const panel = api.getPanel(EXPLORER_PANEL_ID);
  if (!panel) addExplorer(api).api.setActive();
  else if (panel.api.isActive) panel.api.close();
  else panel.api.setActive();
}

export function restoreDefaultLayout() {
  if (!api) return;
  api.getPanel(EXPLORER_PANEL_ID)?.api.close();
  if (!editorAnchor(api)) addWelcome(api);
  addExplorer(api);
}

export function Workbench() {
  const onReady = ({ api: dockview }: DockviewReadyEvent) => {
    api = dockview;
    attachDockview(dockview);

    try {
      const saved = localStorage.getItem(LAYOUT_STORAGE_KEY);
      if (saved) dockview.fromJSON(JSON.parse(saved));
      else defaultLayout(dockview);
    } catch {
      dockview.clear();
      defaultLayout(dockview);
    }
    // Drop tabs whose console did not survive (for example, cleared storage).
    for (const panel of consolePanels(dockview)) {
      if (!useConsoles.getState().consoles[panel.id.slice("console:".length)]) panel.api.close();
    }
    if (!editorAnchor(dockview)) addWelcome(dockview);

    dockview.onDidActivePanelChange(({ panel }) => {
      if (panel?.id.startsWith("console:")) {
        useWorkbench.setState({ activeConsoleId: panel.id.slice("console:".length) });
      }
    });

    dockview.onDidRemovePanel((panel) => {
      if (!panel.id.startsWith("console:")) return;
      const consoleId = panel.id.slice("console:".length);
      void useConsoles.getState().remove(consoleId);
      if (useWorkbench.getState().activeConsoleId === consoleId) useWorkbench.setState({ activeConsoleId: undefined });
      // Keep an editor area around so new consoles have a home.
      if (!editorAnchor(dockview)) addWelcome(dockview);
    });

    let pending = 0;
    dockview.onDidLayoutChange(() => {
      window.clearTimeout(pending);
      pending = window.setTimeout(() => {
        try {
          localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(dockview.toJSON()));
        } catch {
          // Layout not persisted; the default is restored next launch.
        }
      }, 300);
    });
  };

  return <DockviewReact className="h-full" theme={theme} components={components} onReady={onReady} />;
}
