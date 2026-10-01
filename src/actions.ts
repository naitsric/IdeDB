import { effectiveSchema, useConsoles } from "./db/consoles";
import { useDataSources } from "./db/dataSources";
import { selectAll, tableQuery } from "./db/sql";
import { newDataSource, useDialogs } from "./dialogs/dialogs";
import { catalogFor } from "./editor/catalog";
import { declarationAt } from "./editor/navigation";
import { editorFor } from "./editor/registry";
import { splitStatements, type Statement } from "./editor/statements";
import type { RevealRequest } from "./explorer/reveal";
import { useExplorerSelection } from "./explorer/selection";
import { showConsole, useWorkbench } from "./workbench/bridge";

/**
 * User-level actions shared by commands, context menus and Search
 * Everywhere, so each behaves the same wherever it is triggered.
 */

/** The data source an action without an explicit target applies to. */
export function contextDataSourceId(): string | undefined {
  const selection = useExplorerSelection.getState().selection;
  if (selection) return selection.sourceId;
  const { activeConsoleId } = useWorkbench.getState();
  if (activeConsoleId) return useConsoles.getState().consoles[activeConsoleId]?.dataSourceId;
  return useDataSources.getState().sources[0]?.id;
}

export function createDataSource() {
  useDialogs.getState().openDataSource(newDataSource());
}

export function editDataSource(sourceId: string) {
  const source = useDataSources.getState().sources.find((s) => s.id === sourceId);
  if (source) useDialogs.getState().openDataSource(source);
}

/** Opens a new console on a data source, with `sql` in its editor (not run). */
export function newConsole(sourceId = contextDataSourceId(), sql?: string) {
  if (!sourceId) {
    createDataSource();
    return;
  }
  showConsole(useConsoles.getState().create(sourceId, sql));
}

/**
 * Opens (or refocuses) the console showing a table's rows and loads them.
 * `where` replaces the filter bar's condition, e.g. to show the row a
 * foreign key points to. When the rows being replaced have unsubmitted
 * edits, the user is asked first, before anything changes.
 */
export async function openTableData(sourceId: string, schema: string, table: string, options: { where?: string } = {}) {
  const explorer = useDataSources.getState().explorers[sourceId];
  const source = useDataSources.getState().sources.find((s) => s.id === sourceId);
  if (!source) return;
  const sql = selectAll(source.params.engine, schema, table, explorer?.server?.defaultSchema ?? null);
  const consoles = useConsoles.getState();
  const existing = Object.values(consoles.consoles).find(
    (c) => c.dataSourceId === sourceId && c.table?.schema === schema && c.table.name === table,
  );
  const consoleId = existing?.id ?? consoles.create(sourceId, sql, { schema, name: table });
  showConsole(consoleId);
  if (!(await consoles.confirmReplace(consoleId))) return;
  if (options.where !== undefined) consoles.setTableFilter(consoleId, { where: options.where });
  await runTableQuery(consoleId, { confirmed: true });
}

/**
 * Reloads a table console's data with its filter bar applied. Runs the table
 * query itself rather than the editor text, which the user may have changed,
 * and marks the result as the table's data so it can be edited.
 */
export function runTableQuery(consoleId: string, options: { confirmed?: boolean } = {}) {
  const entry = useConsoles.getState().consoles[consoleId];
  const table = entry?.table;
  const source = useDataSources.getState().sources.find((s) => s.id === entry?.dataSourceId);
  if (!table || !source) return;
  const sql = tableQuery(source.params.engine, table.schema, table.name, effectiveSchema(entry), table);
  return useConsoles.getState().runStatement(consoleId, sql, {
    table: { schema: table.schema, name: table.name },
    confirmed: options.confirmed,
  });
}

/**
 * Runs statements one after another, each in its own result tab (the first
 * reuses the active tab unless pinned). Stops at the first failure and
 * marks it in the editor.
 */
export async function runStatements(consoleId: string, statements: readonly Statement[]) {
  const editor = editorFor(consoleId);
  editor?.clearErrors();
  for (const [i, statement] of statements.entries()) {
    editor?.flash(statement);
    const result = await useConsoles.getState().runStatement(consoleId, statement.text, { newTab: i > 0 });
    if (result?.status === "error") editor?.showError(statement, result.error ?? "Error", result.errorPosition);
    if (result?.status !== "done") return;
  }
}

/** What the name under a console editor's caret refers to, as an explorer path. */
export async function declarationUnderCaret(consoleId: string): Promise<RevealRequest | null> {
  const editor = editorFor(consoleId);
  const entry = useConsoles.getState().consoles[consoleId];
  const source = useDataSources.getState().sources.find((s) => s.id === entry?.dataSourceId);
  if (!editor || !entry || !source) return null;
  const { state, pos } = editor.caret();
  const declaration = await declarationAt(state, pos, catalogFor(consoleId, source.id, source.params.engine));
  return declaration && { sourceId: source.id, ...declaration };
}

/** What ⌘⏎ runs in a console: the editor's selection or caret statement. */
export function statementsToRun(consoleId: string): Statement[] {
  const editor = editorFor(consoleId);
  if (editor) return editor.statementsToRun();
  // No editor mounted (tab never shown): fall back to the whole text.
  const entry = useConsoles.getState().consoles[consoleId];
  const engine = useDataSources.getState().sources.find((s) => s.id === entry?.dataSourceId)?.params.engine;
  return entry ? splitStatements(entry.sql, engine) : [];
}
