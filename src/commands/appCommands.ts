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
import { LOCALES, t, useLanguage, type MessageKey } from "../i18n";
import { editorFor } from "../editor/registry";
import { revealInExplorer } from "../explorer/reveal";
import { activeResult, isRunning, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { useFetchSettings } from "../db/fetching";
import { qualifiedName, quoteIdent } from "../db/sql";
import {
  confirmEndTransactions,
  consolesWithOpenTransactions,
  endTransaction,
  quitApp,
  toggleMode,
  useTransactions,
} from "../db/transactions";
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
import { openNewClient, showApprovals, toggleServer, useMcp } from "../mcp/store";
import { useTheme, useTranslucentSidebar } from "../theme";
import { closeActivePanel, hasActivePanel, useWorkbench } from "../workbench/bridge";
import { restoreDefaultLayout, showExplorer, showMcp, toggleExplorer, toggleMcp } from "../workbench/Workbench";
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
/** Commit/Rollback: the active console has a transaction open and nothing running on it. */
const canEndTransaction = () => {
  const entry = activeConsole();
  return !!entry?.inTransaction && !isRunning(entry) && !useTransactions.getState().ending[entry.id];
};
/** Asks to end the open transactions of a data source's consoles before its sessions go away. */
const confirmForDataSource = (dataSourceId: string, action: string) =>
  confirmEndTransactions(consolesWithOpenTransactions(useConsoles.getState().consoles, dataSourceId), action);
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
/** The result with rows still open to fetch: the focused grid's, else the active console's. */
const resultWithRest = () => {
  const grid = activeGrid();
  const entry = grid?.entry ?? activeConsole();
  const result = grid?.result ?? activeResult(entry);
  if (!entry || !result || result.more !== "open" || isRunning(entry)) return undefined;
  return { consoleId: entry.id, resultId: result.id };
};

/** Every app-wide command. Panels only render; behavior lives here and in actions.ts. */
export function registerAppCommands(): () => void {
  const commands: Omit<Command, "titleKey" | "title">[] = [
    // Navigate
    {
      id: "search.findAction",
      category: "Navigate",
      keybinding: "$mod+Shift+KeyA",
      keywords: ["command palette"],
      run: () => useSearchEverywhere.getState().show("actions"),
    },
    {
      id: "navigate.table",
      category: "Navigate",
      keybinding: "$mod+KeyO",
      run: () => useSearchEverywhere.getState().show("tables"),
    },

    // Data sources
    {
      id: "datasource.new",
      category: "Data Source",
      keybinding: "$mod+KeyN",
      keywords: ["add", "connection"],
      run: createDataSource,
    },
    {
      id: "datasource.edit",
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
      category: "Data Source",
      enabled: () => !!contextDataSourceId() && !isConnected(contextDataSourceId()),
      run: () => sources().connect(contextDataSourceId()!),
    },
    {
      id: "datasource.disconnect",
      category: "Data Source",
      enabled: () => isConnected(contextDataSourceId()),
      run: async () => {
        const id = contextDataSourceId()!;
        if (await confirmForDataSource(id, "disconnect")) await sources().disconnect(id);
      },
    },
    {
      id: "datasource.delete",
      category: "Data Source",
      enabled: () => selection()?.kind === "dataSource",
      run: async () => {
        const id = selection()?.sourceId;
        const source = sources().sources.find((s) => s.id === id);
        if (!source) return;
        const confirmed = await ask(t("datasource.delete.confirm", { name: source.name }), {
          title: t("command.datasource.delete"),
          kind: "warning",
          okLabel: t("common.delete"),
        });
        if (confirmed && (await confirmForDataSource(source.id, "delete the data source"))) await sources().remove(source.id);
      },
    },

    // Explorer
    {
      id: "explorer.refresh",
      category: "Database Explorer",
      keybinding: "$mod+Alt+KeyY",
      keywords: ["reload", "synchronize", "introspect"],
      enabled: () => !!contextDataSourceId(),
      run: () => sources().refresh(contextDataSourceId()!),
    },
    {
      id: "explorer.openData",
      category: "Database Explorer",
      keybinding: "F4",
      enabled: () => selection()?.kind === "table" || selection()?.kind === "column",
      run: () => {
        const s = selection();
        if (s?.kind === "table" || s?.kind === "column") void openTableData(s.sourceId, s.schema, s.table);
      },
    },
    {
      id: "explorer.copyName",
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
      category: "Database Explorer",
      keywords: ["pg_catalog", "information_schema", "hide"],
      run: () => sources().toggleSystemSchemas(),
    },

    // Console
    {
      id: "console.new",
      category: "Console",
      keybinding: "$mod+Shift+KeyL",
      run: () => newConsole(),
    },
    {
      id: "console.execute",
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
      category: "Console",
      keybinding: "$mod+F2",
      keywords: ["stop", "abort"],
      enabled: () => isRunning(activeConsole()),
      run: () => {
        const entry = activeConsole();
        if (entry) return useConsoles.getState().cancel(entry.id);
      },
    },

    // Transactions
    {
      id: "transaction.commit",
      category: "Transaction",
      keybinding: "$mod+Alt+Enter",
      keywords: ["transaction", "tx"],
      enabled: canEndTransaction,
      run: async () => {
        const entry = activeConsole();
        if (entry) await endTransaction(entry.id, "commit");
      },
    },
    {
      id: "transaction.rollback",
      category: "Transaction",
      keybinding: "$mod+Alt+Shift+KeyZ",
      keywords: ["transaction", "tx", "undo"],
      enabled: canEndTransaction,
      run: async () => {
        const entry = activeConsole();
        if (entry) await endTransaction(entry.id, "rollback");
      },
    },
    {
      id: "transaction.toggleMode",
      category: "Transaction",
      keywords: ["tx", "auto-commit", "autocommit", "auto", "manual"],
      enabled: () => !!activeConsole(),
      run: () => {
        const entry = activeConsole();
        if (entry) toggleMode(entry.id);
      },
    },

    {
      id: "editor.gotoDeclaration",
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
      category: "Data Editor",
      keybinding: "$mod+Alt+KeyZ",
      context: "grid",
      enabled: () => pendingChanges() > 0,
      run: revertSelected,
    },
    {
      id: "grid.revertAll",
      category: "Data Editor",
      keywords: ["discard"],
      enabled: () => pendingChanges() > 0,
      run: revertAll,
    },
    {
      id: "grid.addRow",
      category: "Data Editor",
      keybinding: "$mod+KeyN",
      context: "grid",
      keywords: ["insert", "new row"],
      enabled: () => !!editing(),
      run: addNewRow,
    },
    {
      id: "grid.duplicateRows",
      category: "Data Editor",
      keybinding: "$mod+KeyD",
      context: "grid",
      keywords: ["clone", "copy row"],
      enabled: () => !!editing() && selectedRowsInActive().length > 0,
      run: duplicateSelectedRows,
    },
    {
      id: "grid.deleteRows",
      category: "Data Editor",
      keybinding: "$mod+Backspace",
      context: "grid",
      keywords: ["remove"],
      enabled: () => !!editing() && selectedRowsInActive().length > 0 && !typingInField(),
      run: deleteSelectedRows,
    },
    {
      id: "grid.setNull",
      category: "Data Editor",
      keybinding: "$mod+Alt+KeyN",
      context: "grid",
      enabled: () => !!editing() && hasGridSelection(),
      run: setSelectedNull,
    },
    {
      id: "grid.filter",
      category: "Data Editor",
      keybinding: "$mod+KeyF",
      context: "grid",
      keywords: ["where"],
      enabled: () => !!activeGrid()?.result.table,
      run: focusFilter,
    },
    {
      id: "grid.goToReferenced",
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
      category: "Results",
      keybinding: "$mod+KeyC",
      context: "grid",
      keywords: ["tsv", "clipboard"],
      enabled: () => hasGridSelection() && !typingInField(),
      run: () => copySelection("tsv"),
    },
    ...(
      [
        ["grid.copyCsv", "csv"],
        ["grid.copyJson", "json"],
        ["grid.copyInserts", "sql"],
      ] as const
    ).map(([id, format]) => ({
      id,
      category: "Results",
      keywords: ["clipboard"],
      enabled: hasGridSelection,
      run: () => copySelection(format),
    })),
    ...(
      [
        ["grid.exportCsv", "csv"],
        ["grid.exportJson", "json"],
        ["grid.exportSql", "sql"],
      ] as const
    ).map(([id, format]) => ({
      id,
      category: "Results",
      keywords: ["save", "file", "download"],
      enabled: () => {
        const result = activeGrid()?.result;
        return !!result && result.columns.length > 0 && result.status !== "running";
      },
      run: () => exportResult(format),
    })),
    {
      id: "results.fetchNext",
      category: "Results",
      keywords: ["more rows", "load", "paginate"],
      enabled: () => resultWithRest() !== undefined,
      run: () => {
        const target = resultWithRest();
        if (target) return useConsoles.getState().fetchMore(target.consoleId, target.resultId);
      },
    },
    {
      id: "results.fetchAll",
      category: "Results",
      keywords: ["more rows", "load", "everything"],
      enabled: () => resultWithRest() !== undefined,
      run: () => {
        const target = resultWithRest();
        if (target) return useConsoles.getState().fetchMore(target.consoleId, target.resultId, true);
      },
    },
    {
      id: "results.closeResultSet",
      category: "Results",
      keywords: ["release", "cursor", "stop fetching"],
      enabled: () => resultWithRest() !== undefined,
      run: () => {
        const target = resultWithRest();
        if (target) return useConsoles.getState().closeCursor(target.consoleId);
      },
    },
    {
      id: "results.pageSize",
      category: "Results",
      keywords: ["fetch size", "rows", "limit", "settings"],
      run: () => useFetchSettings.setState({ dialogOpen: true }),
    },
    {
      id: "grid.valueViewer",
      category: "View",
      keybinding: "$mod+Alt+KeyV",
      keywords: ["cell", "json", "inspect"],
      run: toggleValueViewer,
    },

    // View
    {
      id: "view.toolWindow.explorer",
      category: "View",
      keybinding: "$mod+Digit1",
      run: toggleExplorer,
    },
    {
      id: "view.toolWindow.mcp",
      category: "View",
      // IntelliJ's Services key: the tool window of a server IdeDB runs.
      keybinding: "$mod+Digit8",
      keywords: ["model context protocol", "ai", "llm", "agents", "clients", "tokens", "server"],
      run: toggleMcp,
    },
    {
      id: "view.closeTab",
      category: "Workbench",
      keybinding: "$mod+KeyW",
      enabled: hasActivePanel,
      run: closeActivePanel,
    },
    {
      id: "app.quit",
      category: "Application",
      keybinding: "$mod+KeyQ",
      keywords: ["exit", "close"],
      run: quitApp,
    },
    {
      id: "view.resetLayout",
      category: "View",
      keywords: ["reset", "windows"],
      run: restoreDefaultLayout,
    },

    // MCP server
    {
      id: "mcp.toggleServer",
      category: "MCP",
      keywords: ["start", "stop", "enable", "disable", "ai", "llm", "model context protocol"],
      run: toggleServer,
    },
    {
      id: "mcp.newClient",
      category: "MCP",
      keywords: ["add", "token", "claude", "cursor", "ai", "llm", "agent"],
      run: openNewClient,
    },
    {
      id: "mcp.showActivity",
      category: "MCP",
      keywords: ["audit", "log", "history", "queries", "statements", "ai", "llm", "agent"],
      run: () => showMcp("activity"),
    },
    {
      id: "mcp.showApprovals",
      category: "MCP",
      keywords: ["approve", "reject", "write", "waiting"],
      enabled: () => useMcp.getState().approvals.requests.length > 0,
      run: showApprovals,
    },

    // Appearance
    ...(["system", "light", "dark"] as const).map((preference) => ({
      id: `appearance.theme.${preference}`,
      category: "Appearance",
      keywords: ["color scheme", "dark mode"],
      run: () => useTheme.getState().setPreference(preference),
    })),
    {
      id: "appearance.translucentSidebar",
      category: "Appearance",
      keywords: ["vibrancy", "transparency", "glass"],
      run: () => useTranslucentSidebar.getState().toggle(),
    },

    // Language: System, then one command per translation file.
    {
      id: "appearance.language.system",
      category: "Appearance",
      keywords: ["language", "idioma", "locale", "translation"],
      run: () => useLanguage.getState().setPreference("system"),
    },
  ];

  const localized: Command[] = commands.map((c) => ({ ...c, titleKey: `command.${c.id}` as MessageKey }));
  for (const [code, { name }] of Object.entries(LOCALES)) {
    localized.push({
      id: `appearance.language.${code}`,
      get title() {
        return t("command.appearance.language.named", { language: name });
      },
      category: "Appearance",
      keywords: ["language", "idioma", "locale", "translation"],
      run: () => useLanguage.getState().setPreference(code),
    });
  }

  return registerCommands(localized);
}
