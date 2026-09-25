import { CompactSelection, type GridSelection, type Rectangle } from "@glideapps/glide-data-grid";
import { create } from "zustand";
import type { Engine, TableInfo, TableRef } from "../db/api";
import { useConsoles, type ConsoleState, type ResultMeta } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { NO_EDITS, type PendingEdits } from "./edits";

/**
 * State of the result grids: which one commands act on, their selections,
 * and the data editor's pending changes, keyed by result id.
 */

interface GridsState {
  /** The grid commands act on: the one focused last. */
  active?: { consoleId: string; resultId: number };
  selections: Record<number, GridSelection>;
  edits: Record<number, PendingEdits>;
  submitting: Record<number, boolean>;
  /** Why the last submit failed, and the row of the failing change. */
  failures: Record<number, { message: string; row: number } | undefined>;
  /** A one-off message for the status line, e.g. where an export went. */
  notices: Record<number, string | undefined>;
  valueViewer: boolean;
}

const VIEWER_KEY = "idedb.valueViewer";

function readViewer(): boolean {
  try {
    return localStorage.getItem(VIEWER_KEY) === "1";
  } catch {
    return false;
  }
}

export const EMPTY_SELECTION: GridSelection = { columns: CompactSelection.empty(), rows: CompactSelection.empty() };

export const useGrids = create<GridsState>(() => ({
  selections: {},
  edits: {},
  submitting: {},
  failures: {},
  notices: {},
  valueViewer: readViewer(),
}));

export function toggleValueViewer() {
  const valueViewer = !useGrids.getState().valueViewer;
  useGrids.setState({ valueViewer });
  try {
    localStorage.setItem(VIEWER_KEY, valueViewer ? "1" : "0");
  } catch {
    // Preference not persisted; harmless.
  }
}

export function editsOf(resultId: number): PendingEdits {
  return useGrids.getState().edits[resultId] ?? NO_EDITS;
}

export function setEdits(resultId: number, edits: PendingEdits) {
  useGrids.setState((s) => ({
    edits: { ...s.edits, [resultId]: edits },
    failures: { ...s.failures, [resultId]: undefined },
  }));
}

// Forget the state of results whose tab or console went away.
useConsoles.subscribe((s) => {
  const live = new Set<number>();
  for (const entry of Object.values(s.consoles)) for (const r of entry.results) live.add(r.id);
  const grids = useGrids.getState();
  const stale = (record: Record<number, unknown>) => Object.keys(record).some((id) => !live.has(Number(id)));
  if (!stale(grids.edits) && !stale(grids.selections) && !(grids.active && !live.has(grids.active.resultId))) return;
  const keep = <T,>(record: Record<number, T>) =>
    Object.fromEntries(Object.entries(record).filter(([id]) => live.has(Number(id)))) as Record<number, T>;
  useGrids.setState({
    edits: keep(grids.edits),
    selections: keep(grids.selections),
    submitting: keep(grids.submitting),
    failures: keep(grids.failures),
    notices: keep(grids.notices),
    active: grids.active && live.has(grids.active.resultId) ? grids.active : undefined,
  });
});

/** Whether a table result can be edited, and what edits need to know. */
export type EditorTarget =
  | {
      editable: true;
      table: TableRef;
      info: TableInfo;
      engine: Engine;
      defaultSchema: string | null;
      columnNames: string[];
      /** Result column index of each primary key column, in key order. */
      keyColumns: number[];
    }
  | { editable: false; reason: string };

/** The introspected structure of the table a result shows, once its schema is loaded. */
export function tableInfo(entry: ConsoleState | undefined, result: ResultMeta | undefined): TableInfo | undefined {
  if (!entry || !result?.table) return undefined;
  const load = useDataSources.getState().explorers[entry.dataSourceId]?.models[result.table.schema];
  return load?.state === "loaded" ? load.model.tables.find((t) => t.name === result.table!.name) : undefined;
}

/** `undefined` for results that are not a table's data. */
export function editorTarget(entry: ConsoleState | undefined, result: ResultMeta | undefined): EditorTarget | undefined {
  if (!entry || !result?.table) return undefined;
  const { sources, explorers } = useDataSources.getState();
  const source = sources.find((s) => s.id === entry.dataSourceId);
  const explorer = explorers[entry.dataSourceId];
  const load = explorer?.models[result.table.schema];
  if (!source || load?.state !== "loaded") return { editable: false, reason: "Reading the table structure…" };

  const info = tableInfo(entry, result);
  if (!info) return { editable: false, reason: "Read-only: table not found in the schema" };
  if (info.kind !== "table") return { editable: false, reason: "Read-only: views cannot be edited" };
  if (result.status === "running") return { editable: false, reason: "Rows are still loading" };
  if (result.status === "error") return { editable: false, reason: "Read-only: the query failed" };

  const columnNames = result.columns.map((c) => c.name);
  const keys = info.columns
    .filter((c) => c.primaryKey !== null)
    .sort((a, b) => a.primaryKey! - b.primaryKey!)
    .map((c) => columnNames.indexOf(c.name));
  if (keys.length === 0) return { editable: false, reason: "Read-only: the table has no primary key" };
  if (keys.includes(-1)) return { editable: false, reason: "Read-only: the result does not include the primary key" };

  return {
    editable: true,
    table: result.table,
    info,
    engine: source.params.engine,
    defaultSchema: explorer?.server?.defaultSchema ?? null,
    columnNames,
    keyColumns: keys,
  };
}

/** The console and result the active grid shows, if they still exist. */
export function activeGrid(): { entry: ConsoleState; result: ResultMeta } | undefined {
  const active = useGrids.getState().active;
  if (!active) return undefined;
  const entry = useConsoles.getState().consoles[active.consoleId];
  const result = entry?.results.find((r) => r.id === active.resultId);
  return entry && result ? { entry, result } : undefined;
}

/** Rows touched by a selection: selected rows plus every row of the selected ranges. */
export function selectedRows(selection: GridSelection | undefined): number[] {
  if (!selection) return [];
  const rows = new Set<number>(selection.rows.toArray());
  for (const range of selectionRanges(selection)) {
    for (let r = range.y; r < range.y + range.height; r++) rows.add(r);
  }
  return [...rows].sort((a, b) => a - b);
}

function selectionRanges(selection: GridSelection): readonly Rectangle[] {
  const current = selection.current;
  return current ? [current.range, ...current.rangeStack] : [];
}

/**
 * The cells a copy or aggregate applies to: the current range, else the
 * selected rows (all columns), else the selected columns (all rows).
 */
export function selectionCells(
  selection: GridSelection | undefined,
  rowCount: number,
  width: number,
): { rows: number[]; cols: number[] } | undefined {
  if (!selection) return undefined;
  const range = (from: number, count: number) => Array.from({ length: count }, (_, i) => from + i);
  if (selection.current) {
    const { x, y, width: w, height: h } = selection.current.range;
    return { rows: range(y, h), cols: range(x, w) };
  }
  if (selection.rows.length > 0) return { rows: selection.rows.toArray(), cols: range(0, width) };
  if (selection.columns.length > 0) return { rows: range(0, rowCount), cols: selection.columns.toArray() };
  return undefined;
}
