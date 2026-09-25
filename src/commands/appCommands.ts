import { ask } from "@tauri-apps/plugin-dialog";
import {
  contextDataSourceId,
  createDataSource,
  declarationUnderCaret,
  editDataSource,
  newConsole,
  openTableData,
  runStatements,
  statementsToRun,
} from "../actions";
import { editorFor } from "../editor/registry";
import { revealInExplorer } from "../explorer/reveal";
import { isRunning, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { qualifiedName, quoteIdent } from "../db/sql";
import { useExplorerSelection } from "../explorer/selection";
import {
  addNewRow,
  cellEditorOpen,
  copySelection,
  deleteSelectedRows,
  duplicateSelectedRows,
  editing,
  exportResult,
  focusFilter,
  goToReferencedRow,
  pendingChanges,
  referencedRow,
  revertAll,
  revertSelected,
  setSelectedNull,
  submit,
  typingInField,
} from "../grid/actions";
import { activeGrid, selectedRows, toggleValueViewer, useGrids } from "../grid/dataEditor";
import { useTheme, useTranslucentSidebar } from "../theme";
import { closeActivePanel, hasActivePanel, useWorkbench } from "../workbench/bridge";
import { restoreDefaultLayout, showExplorer, toggleExplorer } from "../workbench/Workbench";
import { useHistoryPalette } from "./HistoryPalette";
import { registerCommands, type Command } from "./registry";
import { useSearchEverywhere } from "./SearchEverywhere";

const sources = () => useDataSources.getState();
const selection = () => useExplorerSelection.getState().selection;
const activeConsole = () => {
  const id = useWorkbench.getState().activeConsoleId;
  return id ? useConsoles.getState().consoles[id] : undefined;
};
const isConnected = (id: string | undefined) => !!id && sources().explorers[id]?.status === "connected";
const hasActiveEditor = () => {
  const entry = activeConsole();
  return !!entry && !!editorFor(entry.id);
};
const hasGridSelection = () => {
  const grid = activeGrid();
  const selection = grid && useGrids.getState().selections[grid.result.id];
  return !!selection && (!!selection.current || selection.rows.length > 0 || selection.columns.length > 0);
};
const selectedRowsInActive = () => {
  const grid = activeGrid();
  return grid ? selectedRows(useGrids.getState().selections[grid.result.id]) : [];
};

/** Every app-wide command. Panels only render; behavior lives here and in actions.ts. */
export function registerAppCommands(): () => void {
  const commands: Command[] = [
    // Navigate
    {
      id: "search.findAction",
      title: "Find Action",
      category: "Navigate",
      keybinding: "$mod+Shift+KeyA",
      keywords: ["command palette"],
      run: () => useSearchEverywhere.getState().show("actions"),
    },
    {
      id: "navigate.table",
      title: "Go to Table",
      category: "Navigate",
      keybinding: "$mod+KeyO",
      run: () => useSearchEverywhere.getState().show("tables"),
    },

    // Data sources
    {
      id: "datasource.new",
      title: "New Data Source",
      category: "Data Source",
      keybinding: "$mod+KeyN",
      keywords: ["add", "connection"],
      run: createDataSource,
    },
    {
      id: "datasource.edit",
      title: "Data Source Properties",
      category: "Data Source",
      keybinding: "$mod+Semicolon",
      keywords: ["edit"],
      enabled: () => !!contextDataSourceId(),
      run: () => {
        const id = contextDataSourceId();
        if (id) editDataSource(id);
      },
    },
    {
      id: "datasource.connect",
      title: "Connect",
      category: "Data Source",
      enabled: () => !!contextDataSourceId() && !isConnected(contextDataSourceId()),
      run: () => sources().connect(contextDataSourceId()!),
    },
    {
      id: "datasource.disconnect",
      title: "Disconnect",
      category: "Data Source",
      enabled: () => isConnected(contextDataSourceId()),
      run: () => sources().disconnect(contextDataSourceId()!),
    },
    {
      id: "datasource.delete",
      title: "Delete Data Source",
      category: "Data Source",
      enabled: () => selection()?.kind === "dataSource",
      run: async () => {
        const id = selection()?.sourceId;
        const source = sources().sources.find((s) => s.id === id);
        if (!source) return;
        const confirmed = await ask(`Delete data source "${source.name}"? Its saved password is removed too.`, {
          title: "Delete Data Source",
          kind: "warning",
          okLabel: "Delete",
        });
        if (confirmed) await sources().remove(source.id);
      },
    },

    // Explorer
    {
      id: "explorer.refresh",
      title: "Refresh",
      category: "Database Explorer",
      keybinding: "$mod+Alt+KeyY",
      keywords: ["reload", "synchronize", "introspect"],
      enabled: () => !!contextDataSourceId(),
      run: () => sources().refresh(contextDataSourceId()!),
    },
    {
      id: "explorer.openData",
      title: "Open Table Data",
      category: "Database Explorer",
      keybinding: "F4",
      enabled: () => selection()?.kind === "table" || selection()?.kind === "column",
      run: () => {
        const s = selection();
        if (s?.kind === "table" || s?.kind === "column") openTableData(s.sourceId, s.schema, s.table);
      },
    },
    {
      id: "explorer.copyName",
      title: "Copy Qualified Name",
      category: "Database Explorer",
      keybinding: "$mod+Alt+Shift+KeyC",
      enabled: () => selection()?.kind === "table" || selection()?.kind === "column",
      run: async () => {
        const s = selection();
        const source = sources().sources.find((x) => x.id === s?.sourceId);
        if (!source || (s?.kind !== "table" && s?.kind !== "column")) return;
        const engine = source.params.engine;
        const table = qualifiedName(engine, s.schema, s.table, null);
        await navigator.clipboard.writeText(s.kind === "column" ? `${table}.${quoteIdent(engine, s.column)}` : table);
      },
    },
    {
      id: "explorer.toggleSystemSchemas",
      title: "Show System Schemas",
      category: "Database Explorer",
      keywords: ["pg_catalog", "information_schema", "hide"],
      run: () => sources().toggleSystemSchemas(),
    },

    // Console
    {
      id: "console.new",
      title: "New Query Console",
      category: "Console",
      keybinding: "$mod+Shift+KeyL",
      run: () => newConsole(),
    },
    {
      id: "console.execute",
      title: "Execute Statement",
      category: "Console",
      keybinding: "$mod+Enter",
      keywords: ["run", "query"],
      enabled: () => {
        const entry = activeConsole();
        return !!entry && !entry.connecting && !isRunning(entry);
      },
      run: () => {
        const entry = activeConsole();
        if (entry) return runStatements(entry.id, statementsToRun(entry.id));
      },
    },
    {
      id: "console.cancel",
      title: "Cancel Running Statement",
      category: "Console",
      keybinding: "$mod+F2",
      keywords: ["stop", "abort"],
      enabled: () => isRunning(activeConsole()),
      run: () => {
        const entry = activeConsole();
        if (entry) return useConsoles.getState().cancel(entry.id);
      },
    },

    {
      id: "editor.gotoDeclaration",
      title: "Go to Declaration",
      category: "Navigate",
      keybinding: "$mod+KeyB",
      context: "editor",
      keywords: ["navigate", "explorer", "table", "column"],
      enabled: hasActiveEditor,
      run: async () => {
        const entry = activeConsole();
        const target = entry && (await declarationUnderCaret(entry.id));
        if (!target) return;
        showExplorer();
        revealInExplorer(target);
      },
    },
    {
      id: "editor.reformat",
      title: "Reformat Code",
      category: "Console",
      keybinding: "$mod+Alt+KeyL",
      context: "editor",
      keywords: ["format", "pretty", "beautify", "indent"],
      enabled: hasActiveEditor,
      run: () => {
        const entry = activeConsole();
        if (entry) editorFor(entry.id)?.reformat();
      },
    },
    {
      id: "console.history",
      title: "Query History",
      category: "Console",
      keybinding: "$mod+Alt+KeyE",
      keywords: ["recent", "previous", "queries"],
      enabled: () => !!activeConsole(),
      run: () => {
        const entry = activeConsole();
        if (entry) useHistoryPalette.getState().show(entry.id);
      },
    },

    // Data editor: a table's rows in a console
    {
      id: "grid.submit",
      title: "Submit Changes",
      category: "Data Editor",
      keybinding: "$mod+Enter",
      context: "grid",
      keywords: ["commit", "save", "apply"],
      enabled: () => {
        const grid = activeGrid();
        return (
          !!editing() &&
          (pendingChanges() > 0 || cellEditorOpen()) &&
          !useGrids.getState().submitting[grid!.result.id] &&
          !isRunning(grid!.entry)
        );
      },
      run: submit,
    },
    {
      id: "grid.revertSelected",
      title: "Revert Selected Changes",
      category: "Data Editor",
      keybinding: "$mod+Alt+KeyZ",
      context: "grid",
      enabled: () => pendingChanges() > 0,
      run: revertSelected,
    },
    {
      id: "grid.revertAll",
      title: "Revert All Changes",
      category: "Data Editor",
      keywords: ["discard"],
      enabled: () => pendingChanges() > 0,
      run: revertAll,
    },
    {
      id: "grid.addRow",
      title: "Add Row",
      category: "Data Editor",
      keybinding: "$mod+KeyN",
      context: "grid",
      keywords: ["insert", "new row"],
      enabled: () => !!editing(),
      run: addNewRow,
    },
    {
      id: "grid.duplicateRows",
      title: "Duplicate Rows",
      category: "Data Editor",
      keybinding: "$mod+KeyD",
      context: "grid",
      keywords: ["clone", "copy row"],
      enabled: () => !!editing() && selectedRowsInActive().length > 0,
      run: duplicateSelectedRows,
    },
    {
      id: "grid.deleteRows",
      title: "Delete Rows",
      category: "Data Editor",
      keybinding: "$mod+Backspace",
      context: "grid",
      keywords: ["remove"],
      enabled: () => !!editing() && selectedRowsInActive().length > 0 && !typingInField(),
      run: deleteSelectedRows,
    },
    {
      id: "grid.setNull",
      title: "Set NULL",
      category: "Data Editor",
      keybinding: "$mod+Alt+KeyN",
      context: "grid",
      enabled: () => !!editing() && hasGridSelection(),
      run: setSelectedNull,
    },
    {
      id: "grid.filter",
      title: "Filter Rows",
      category: "Data Editor",
      keybinding: "$mod+KeyF",
      context: "grid",
      keywords: ["where"],
      enabled: () => !!activeGrid()?.result.table,
      run: focusFilter,
    },
    {
      id: "grid.goToReferenced",
      title: "Go to Referenced Row",
      category: "Data Editor",
      keybinding: "$mod+KeyB",
      context: "grid",
      keywords: ["foreign key", "navigate"],
      enabled: () => !!referencedRow(),
      run: goToReferencedRow,
    },

    // Results
    {
      id: "grid.copy",
      title: "Copy",
      category: "Results",
      keybinding: "$mod+KeyC",
      context: "grid",
      keywords: ["tsv", "clipboard"],
      enabled: () => hasGridSelection() && !typingInField(),
      run: () => copySelection("tsv"),
    },
    ...(
      [
        ["grid.copyCsv", "Copy as CSV", "csv"],
        ["grid.copyJson", "Copy as JSON", "json"],
        ["grid.copyInserts", "Copy as SQL INSERT", "sql"],
      ] as const
    ).map(([id, title, format]) => ({
      id,
      title,
      category: "Results",
      keywords: ["clipboard"],
      enabled: hasGridSelection,
      run: () => copySelection(format),
    })),
    ...(
      [
        ["grid.exportCsv", "Export Data to CSV…", "csv"],
        ["grid.exportJson", "Export Data to JSON…", "json"],
        ["grid.exportSql", "Export Data to SQL INSERT…", "sql"],
      ] as const
    ).map(([id, title, format]) => ({
      id,
      title,
      category: "Results",
      keywords: ["save", "file", "download"],
      enabled: () => {
        const result = activeGrid()?.result;
        return !!result && result.columns.length > 0 && result.status !== "running";
      },
      run: () => exportResult(format),
    })),
    {
      id: "grid.valueViewer",
      title: "Value Viewer",
      category: "View",
      keybinding: "$mod+Alt+KeyV",
      keywords: ["cell", "json", "inspect"],
      run: toggleValueViewer,
    },

    // View
    {
      id: "view.toolWindow.explorer",
      title: "Database Explorer",
      category: "View",
      keybinding: "$mod+Digit1",
      run: toggleExplorer,
    },
    {
      id: "view.closeTab",
      title: "Close Tab",
      category: "Workbench",
      keybinding: "$mod+KeyW",
      enabled: hasActivePanel,
      run: closeActivePanel,
    },
    {
      id: "view.resetLayout",
      title: "Restore Default Layout",
      category: "View",
      keywords: ["reset", "windows"],
      run: restoreDefaultLayout,
    },

    // Appearance
    ...(["system", "light", "dark"] as const).map((preference) => ({
      id: `appearance.theme.${preference}`,
      title: `Theme: ${preference[0].toUpperCase()}${preference.slice(1)}`,
      category: "Appearance",
      keywords: ["color scheme", "dark mode"],
      run: () => useTheme.getState().setPreference(preference),
    })),
    {
      id: "appearance.translucentSidebar",
      title: "Toggle Translucent Sidebar",
      category: "Appearance",
      keywords: ["vibrancy", "transparency", "glass"],
      run: () => useTranslucentSidebar.getState().toggle(),
    },
  ];

  return registerCommands(commands);
}
