import type { DataEditorRef, GridSelection } from "@glideapps/glide-data-grid";
import { openTableData } from "../actions";
import { api, errorMessage, type Value } from "../db/api";
import { effectiveSchema, rowGetter, useConsoles, type ConsoleState, type ResultMeta } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { quoteIdent, sqlLiteral } from "../db/sql";
import { beginIfManual, recordSubmit } from "../db/transactions";
import {
  activeGrid,
  editorTarget,
  editsOf,
  EMPTY_SELECTION,
  selectedRows,
  selectionCells,
  setEdits,
  tableInfo,
  useGrids,
} from "./dataEditor";
import {
  addRow,
  afterSubmit,
  cellValue,
  changeCount,
  commitRows,
  DEFAULT,
  deleteRows,
  duplicateRows,
  generatedColumns,
  NO_EDITS,
  revertRows,
  setCell,
  toChanges,
} from "./edits";
import { EXPORT_EXTENSION, toCsv, toInserts, toJson, toJsonObjects, toTsv, type ExportFormat } from "./format";

/**
 * Data editor and result grid actions. Commands call these for the active
 * grid; the grid calls them for its own cells.
 */

/** Mounted grids, to move the selection and scroll after an action. */
export const gridRefs = new Map<number, DataEditorRef>();

/** Filter bar WHERE inputs by console, for "Filter Rows". */
export const filterInputs = new Map<string, HTMLInputElement>();

function engineOf(entry: ConsoleState) {
  const source = useDataSources.getState().sources.find((s) => s.id === entry.dataSourceId);
  return source ? { engine: source.params.engine, defaultSchema: effectiveSchema(entry) } : undefined;
}

/** Loaded rows plus rows added in the editor. */
export function visibleRowCount(result: ResultMeta): number {
  return result.rowCount + editsOf(result.id).inserted.length;
}

function select(resultId: number, selection: GridSelection) {
  useGrids.setState((s) => ({ selections: { ...s.selections, [resultId]: selection } }));
}

/** Selects one cell, scrolls it into view and gives the grid focus. */
function selectCell(resultId: number, col: number, row: number) {
  const rect = { x: col, y: row, width: 1, height: 1 };
  select(resultId, { ...EMPTY_SELECTION, current: { cell: [col, row], range: rect, rangeStack: [] } });
  // A row just added only exists once the grid has re-rendered.
  requestAnimationFrame(() => {
    const grid = gridRefs.get(resultId);
    grid?.scrollTo(col, row);
    grid?.focus();
  });
}

/** The active grid when it can be edited, with what editing needs. */
export function editing() {
  const grid = activeGrid();
  const target = grid && editorTarget(grid.entry, grid.result);
  if (!grid || !target?.editable) return undefined;
  return { ...grid, target, edits: editsOf(grid.result.id), getRow: rowGetter(grid.result.id) };
}

export function pendingChanges(): number {
  const grid = activeGrid();
  return grid ? changeCount(editsOf(grid.result.id)) : 0;
}

/**
 * Edits a cell. Text typed into a binary column as `0x…` hex (how the grid
 * shows bytes) becomes bytes again rather than the characters themselves.
 */
export function editCell(consoleId: string, resultId: number, row: number, col: number, value: Value) {
  const result = useConsoles.getState().consoles[consoleId]?.results.find((r) => r.id === resultId);
  if (!result || useGrids.getState().submitting[resultId]) return;
  const getRow = rowGetter(resultId);
  const binary =
    getRow(row)?.[col] instanceof Uint8Array ||
    /^(bytea|(tiny|medium|long)?blob|(var)?binary)/i.test(result.columns[col]?.typeName ?? "");
  const stored =
    typeof value === "string" && binary && /^0x([0-9a-f]{2})*$/i.test(value) ? hexBytes(value.slice(2)) : value;
  setEdits(resultId, setCell(editsOf(resultId), result.rowCount, getRow, row, col, stored));
}

function hexBytes(hex: string): Uint8Array {
  return Uint8Array.from({ length: hex.length / 2 }, (_, i) => Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16));
}

export function addNewRow() {
  const ctx = editing();
  if (!ctx) return;
  setEdits(ctx.result.id, addRow(ctx.edits, ctx.result.columns.length));
  selectCell(ctx.result.id, 0, visibleRowCount(ctx.result) - 1);
}

/** Duplicates the selected rows, leaving what the database generates (ids, computed columns) to it. */
export function duplicateSelectedRows() {
  const ctx = editing();
  if (!ctx) return;
  const rows = selectedRows(useGrids.getState().selections[ctx.result.id]);
  if (rows.length === 0) return;
  const generated = generatedColumns(ctx.target.columnNames, ctx.target.info.columns);
  const width = ctx.result.columns.length;
  setEdits(ctx.result.id, duplicateRows(ctx.edits, ctx.result.rowCount, ctx.getRow, rows, width, generated));
  selectCell(ctx.result.id, 0, visibleRowCount(ctx.result) - 1);
}

export function deleteSelectedRows() {
  const ctx = editing();
  if (!ctx) return;
  const rows = selectedRows(useGrids.getState().selections[ctx.result.id]);
  if (rows.length === 0) return;
  setEdits(ctx.result.id, deleteRows(ctx.edits, ctx.result.rowCount, rows));
}

export function setSelectedNull() {
  const ctx = editing();
  if (!ctx) return;
  const selection = useGrids.getState().selections[ctx.result.id];
  const cells = selectionCells(selection, visibleRowCount(ctx.result), ctx.result.columns.length);
  if (!cells) return;
  let edits = ctx.edits;
  for (const row of cells.rows) {
    for (const col of cells.cols) edits = setCell(edits, ctx.result.rowCount, ctx.getRow, row, col, null);
  }
  setEdits(ctx.result.id, edits);
}

/** The active grid, unless a submit is running on it (its edits are being written). */
function idleGrid() {
  const grid = activeGrid();
  return grid && !useGrids.getState().submitting[grid.result.id] ? grid : undefined;
}

export function revertSelected() {
  const grid = idleGrid();
  if (!grid) return;
  const rows = selectedRows(useGrids.getState().selections[grid.result.id]);
  setEdits(grid.result.id, revertRows(editsOf(grid.result.id), grid.result.rowCount, rows));
}

export function revertAll() {
  const grid = idleGrid();
  if (grid) setEdits(grid.result.id, NO_EDITS);
}

/** Whether the user is typing in a text field, where keys like ⌘C belong to the field. */
export function typingInField(): boolean {
  return !!document.activeElement?.closest("input, textarea, [contenteditable]");
}

/** Whether a cell's in-grid editor is open (Glide mounts it in #portal). */
export function cellEditorOpen(): boolean {
  return !!document.getElementById("portal")?.contains(document.activeElement);
}

/** Commits an open in-grid cell editor, as clicking outside it would. */
function commitCellEditor() {
  if (cellEditorOpen()) document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
}

/**
 * Writes all pending changes as one unit. On success the loaded rows take
 * what the database stored, in place, so the grid keeps its scroll; on
 * failure nothing is written and the failing row is selected. Inside a
 * transaction the user opened, the changes join it without committing.
 */
export async function submit() {
  commitCellEditor();
  const ctx = editing();
  const sessionId = ctx?.entry.sessionId;
  if (!ctx || changeCount(ctx.edits) === 0 || useGrids.getState().submitting[ctx.result.id]) return;
  const resultId = ctx.result.id;
  const fail = (message: string, row: number) =>
    useGrids.setState((s) => ({ failures: { ...s.failures, [resultId]: { message, row } } }));
  if (sessionId === undefined) {
    fail("Not connected: run the query again to reconnect.", -1);
    return;
  }

  const submitted = ctx.edits;
  const rowCount = ctx.result.rowCount;
  const { changes, targets } = toChanges(submitted, ctx.getRow, ctx.target.columnNames, ctx.target.keyColumns);
  useGrids.setState((s) => ({ submitting: { ...s.submitting, [resultId]: true } }));
  try {
    // Manual transaction mode: the changes go into a transaction, never straight to disk.
    const refused = await beginIfManual(ctx.entry.id, sessionId);
    if (refused) {
      fail(refused, -1);
      return;
    }
    const outcome = await api.apply(sessionId, ctx.target.table, changes);
    if (outcome.status === "applied") {
      useConsoles.getState().mutateRows(ctx.entry.id, resultId, (rows) => commitRows(rows, submitted, targets, outcome.rows));
      setEdits(resultId, afterSubmit(editsOf(resultId), submitted, rowCount));
      useConsoles.getState().setInTransaction(ctx.entry.id, outcome.inTransaction);
      if (outcome.inTransaction) {
        recordSubmit(ctx.entry.id);
        const n = changes.length;
        useGrids.setState((s) => ({
          notices: {
            ...s.notices,
            [resultId]: `${n} ${n === 1 ? "change" : "changes"} applied inside the open transaction, not committed: COMMIT or ROLLBACK decides`,
          },
        }));
      }
      // Deleted rows shift the ones below, so the old selection would point elsewhere.
      if (submitted.deleted.size > 0) select(resultId, EMPTY_SELECTION);
    } else {
      const target = targets[outcome.index];
      const row = target.kind === "insert" ? ctx.result.rowCount + target.index : target.row;
      fail(outcome.message, row);
      selectCell(resultId, 0, row);
    }
  } catch (e) {
    fail(errorMessage(e), -1);
  } finally {
    useGrids.setState((s) => ({ submitting: { ...s.submitting, [resultId]: false } }));
  }
}

/** Selected cells, pending edits included, with their column names. */
function selectedValues(entry: ConsoleState, result: ResultMeta) {
  const selection = useGrids.getState().selections[result.id];
  const cells = selectionCells(selection, visibleRowCount(result), result.columns.length);
  if (!cells) return undefined;
  const edits = editsOf(result.id);
  const getRow = rowGetter(result.id);
  const rows = cells.rows.map((row) =>
    cells.cols.map((col) => {
      const { value } = cellValue(edits, result.rowCount, getRow, row, col);
      return value === DEFAULT ? null : value;
    }),
  );
  return { rows, columns: cells.cols.map((c) => result.columns[c].name), entry };
}

type CopyFormat = "tsv" | "csv" | "json" | "sql";

export async function copySelection(format: CopyFormat) {
  const grid = activeGrid();
  const selected = grid && selectedValues(grid.entry, grid.result);
  const engine = grid && engineOf(grid.entry);
  if (!grid || !selected || !engine) return;
  const { rows, columns } = selected;
  const text = {
    tsv: () => toTsv(rows),
    csv: () => toCsv(rows, columns),
    json: () => toJson(rows, columns),
    sql: () => toInserts(engine.engine, grid.result.table ?? null, engine.defaultSchema, columns, rows),
  }[format]();
  await navigator.clipboard.writeText(text);
}

const EXPORT_LABEL: Record<ExportFormat, string> = { tsv: "TSV", csv: "CSV", json: "JSON", sql: "SQL INSERT statements" };
const EXPORT_CHUNK_ROWS = 5000;

/** Writes every loaded row of the active result to a file the user picks, in chunks. */
export async function exportResult(format: ExportFormat) {
  const grid = activeGrid();
  const engine = grid && engineOf(grid.entry);
  if (!grid || !engine) return;
  const { result } = grid;
  const extension = EXPORT_EXTENSION[format];
  const notice = (message: string) => useGrids.setState((s) => ({ notices: { ...s.notices, [result.id]: message } }));
  let target: { token: number; fileName: string } | null;
  try {
    target = await api.exportBegin(`${result.table?.name ?? "result"}.${extension}`, EXPORT_LABEL[format], extension);
  } catch (e) {
    notice(`Export failed: ${errorMessage(e)}`);
    return;
  }
  if (!target) return;

  const getRow = rowGetter(result.id);
  const columns = result.columns.map((c) => c.name);
  const total = result.rowCount;
  try {
    for (let from = 0; from < Math.max(total, 1); from += EXPORT_CHUNK_ROWS) {
      const rows: Value[][] = [];
      for (let i = from; i < Math.min(from + EXPORT_CHUNK_ROWS, total); i++) rows.push(getRow(i) ?? []);
      const first = from === 0;
      const last = from + EXPORT_CHUNK_ROWS >= total;
      let chunk: string;
      switch (format) {
        case "tsv":
        case "csv": {
          const body = format === "tsv" ? toTsv(rows, first ? columns : undefined) : toCsv(rows, first ? columns : undefined);
          chunk = (first ? "" : "\n") + body + (last ? "\n" : "");
          break;
        }
        case "json": {
          const objects = toJsonObjects(rows, columns);
          chunk = (first ? "[" : ",") + (objects.length ? `\n  ${objects.join(",\n  ")}` : "") + (last ? "\n]\n" : "");
          break;
        }
        case "sql":
          chunk = (rows.length ? toInserts(engine.engine, result.table ?? null, engine.defaultSchema, columns, rows) + "\n" : "");
          break;
      }
      await api.exportWrite(target.token, chunk);
    }
    notice(`Exported ${total.toLocaleString("en-US")} rows to ${target.fileName}`);
  } catch (e) {
    notice(`Export failed: ${errorMessage(e)}`);
  } finally {
    void api.exportFinish(target.token).catch(() => {});
  }
}

/** Opens the table a foreign key points to, filtered to the referenced row. */
export function goToReferencedRow() {
  const target = referencedRow();
  if (target) void openTableData(target.sourceId, target.schema, target.table, { where: target.where });
}

/** The row the selected foreign key cell points to, as a filter on the referenced table. */
export function referencedRow() {
  const grid = activeGrid();
  const cell = grid && useGrids.getState().selections[grid.result.id]?.current?.cell;
  const info = grid && tableInfo(grid.entry, grid.result);
  const engine = grid && engineOf(grid.entry);
  if (!grid || !cell || !info || !engine) return undefined;
  const [col, row] = cell;
  const column = grid.result.columns[col]?.name;
  const fk = info.foreignKeys.find((f) => f.columns.includes(column));
  if (!fk) return undefined;

  const edits = editsOf(grid.result.id);
  const getRow = rowGetter(grid.result.id);
  const conditions: string[] = [];
  for (const [i, local] of fk.columns.entries()) {
    const index = grid.result.columns.findIndex((c) => c.name === local);
    const value = index < 0 ? DEFAULT : cellValue(edits, grid.result.rowCount, getRow, row, index).value;
    // A NULL foreign key references nothing.
    if (value === DEFAULT || value === null) return undefined;
    conditions.push(`${quoteIdent(engine.engine, fk.referencedColumns[i])} = ${sqlLiteral(engine.engine, value)}`);
  }
  return {
    sourceId: grid.entry.dataSourceId,
    schema: fk.referencedSchema,
    table: fk.referencedTable,
    where: conditions.join(" and "),
  };
}

export function focusFilter() {
  const grid = activeGrid();
  const input = grid && filterInputs.get(grid.entry.id);
  input?.focus();
  input?.select();
}
