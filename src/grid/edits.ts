import type { ColumnValue, Row, RowChange, Value } from "../db/api";

/**
 * Pending data editor changes for one result, kept apart from the loaded
 * rows until Submit. All functions are pure: they return new state.
 *
 * Row indexes below `rowCount` address loaded rows; indexes from
 * `rowCount` on address inserted rows, shown after the loaded ones.
 */

/** An inserted-row cell left to the column's default. */
export const DEFAULT = Symbol("default");
export type CellInput = Value | typeof DEFAULT;

export type RowState = "clean" | "modified" | "inserted" | "deleted";

export interface InsertedRow {
  values: CellInput[];
}

export interface PendingEdits {
  /** Changed cells of loaded rows: row → column → new value. */
  updates: Readonly<Record<number, Readonly<Record<number, Value>>>>;
  /** Loaded rows marked for deletion. */
  deleted: ReadonlySet<number>;
  inserted: readonly InsertedRow[];
}

export const NO_EDITS: PendingEdits = { updates: {}, deleted: new Set(), inserted: [] };

type GetRow = (index: number) => Value[] | undefined;

export function changeCount(edits: PendingEdits): number {
  let updated = 0;
  for (const row of Object.keys(edits.updates)) if (!edits.deleted.has(Number(row))) updated++;
  return updated + edits.deleted.size + edits.inserted.length;
}

export function rowState(edits: PendingEdits, rowCount: number, row: number): RowState {
  if (row >= rowCount) return "inserted";
  if (edits.deleted.has(row)) return "deleted";
  return edits.updates[row] ? "modified" : "clean";
}

/** The value a cell shows, pending edits included, and whether it is edited. */
export function cellValue(
  edits: PendingEdits,
  rowCount: number,
  getRow: GetRow,
  row: number,
  col: number,
): { value: CellInput; edited: boolean } {
  if (row >= rowCount) return { value: edits.inserted[row - rowCount]?.values[col] ?? DEFAULT, edited: true };
  const updated = edits.updates[row];
  if (updated && col in updated) return { value: updated[col], edited: true };
  return { value: getRow(row)?.[col] ?? null, edited: false };
}

/** Text a user would type to produce `value`; how edits compare with originals. */
export function editText(value: CellInput): string | null {
  if (value === DEFAULT || value === null) return null;
  if (value instanceof Uint8Array) {
    let out = "0x";
    for (const b of value) out += b.toString(16).padStart(2, "0").toUpperCase();
    return out;
  }
  return String(value);
}

/**
 * Sets a cell. Setting a loaded cell back to its original value drops the
 * edit, so undoing by retyping leaves nothing pending.
 */
export function setCell(
  edits: PendingEdits,
  rowCount: number,
  getRow: GetRow,
  row: number,
  col: number,
  value: Value,
): PendingEdits {
  if (row >= rowCount) {
    const index = row - rowCount;
    const target = edits.inserted[index];
    if (!target) return edits;
    const values = [...target.values];
    values[col] = value;
    return { ...edits, inserted: edits.inserted.map((r, i) => (i === index ? { values } : r)) };
  }
  if (edits.deleted.has(row)) return edits;

  const original = getRow(row)?.[col] ?? null;
  const rowUpdates: Record<number, Value> = { ...edits.updates[row] };
  if (editText(original) === editText(value)) delete rowUpdates[col];
  else rowUpdates[col] = value;

  const updates: Record<number, Record<number, Value>> = { ...edits.updates };
  if (Object.keys(rowUpdates).length > 0) updates[row] = rowUpdates;
  else delete updates[row];
  return { ...edits, updates };
}

export function addRow(edits: PendingEdits, width: number): PendingEdits {
  return { ...edits, inserted: [...edits.inserted, { values: Array<CellInput>(width).fill(DEFAULT) }] };
}

/**
 * Appends copies of rows as new rows. Columns in `resetColumns` (e.g. a
 * generated key) go back to their default so the copy does not collide.
 */
export function duplicateRows(
  edits: PendingEdits,
  rowCount: number,
  getRow: GetRow,
  rows: readonly number[],
  width: number,
  resetColumns: ReadonlySet<number>,
): PendingEdits {
  const copies = rows.map((row) => ({
    values: Array.from({ length: width }, (_, col) =>
      resetColumns.has(col) ? DEFAULT : cellValue(edits, rowCount, getRow, row, col).value,
    ),
  }));
  return { ...edits, inserted: [...edits.inserted, ...copies] };
}

/**
 * Result columns the database fills itself (identity, auto-increment,
 * serial, computed, SQLite's rowid), which a duplicated row leaves unset.
 */
export function generatedColumns(
  columnNames: readonly string[],
  tableColumns: readonly { name: string; generated: boolean }[],
): Set<number> {
  const generated = new Set(tableColumns.filter((c) => c.generated).map((c) => c.name));
  return new Set(columnNames.flatMap((name, col) => (generated.has(name) ? [col] : [])));
}

/** Marks loaded rows deleted; inserted rows are simply dropped. */
export function deleteRows(edits: PendingEdits, rowCount: number, rows: readonly number[]): PendingEdits {
  const deleted = new Set(edits.deleted);
  const dropInserted = new Set<number>();
  for (const row of rows) {
    if (row >= rowCount) dropInserted.add(row - rowCount);
    else deleted.add(row);
  }
  return { ...edits, deleted, inserted: edits.inserted.filter((_, i) => !dropInserted.has(i)) };
}

/** Undoes every pending change on the given rows. */
export function revertRows(edits: PendingEdits, rowCount: number, rows: readonly number[]): PendingEdits {
  const updates: Record<number, Readonly<Record<number, Value>>> = { ...edits.updates };
  const deleted = new Set(edits.deleted);
  const dropInserted = new Set<number>();
  for (const row of rows) {
    if (row >= rowCount) {
      dropInserted.add(row - rowCount);
    } else {
      delete updates[row];
      deleted.delete(row);
    }
  }
  return { updates, deleted, inserted: edits.inserted.filter((_, i) => !dropInserted.has(i)) };
}

/** Where a change came from, to point at the failing row. */
export type ChangeTarget = { kind: "update" | "delete"; row: number } | { kind: "insert"; index: number };

/**
 * The changes to submit, ordered deletes → updates → inserts so a key freed
 * by a delete can be reused by an insert in the same batch.
 */
export function toChanges(
  edits: PendingEdits,
  getRow: GetRow,
  columnNames: readonly string[],
  keyColumns: readonly number[],
): { changes: RowChange[]; targets: ChangeTarget[] } {
  const changes: RowChange[] = [];
  const targets: ChangeTarget[] = [];
  const keyOf = (row: number): ColumnValue[] =>
    keyColumns.map((col) => ({ column: columnNames[col], value: getRow(row)?.[col] ?? null }));

  for (const row of [...edits.deleted].sort((a, b) => a - b)) {
    changes.push({ kind: "delete", key: keyOf(row) });
    targets.push({ kind: "delete", row });
  }
  for (const [rowKey, cols] of Object.entries(edits.updates)) {
    const row = Number(rowKey);
    if (edits.deleted.has(row)) continue;
    const values = Object.entries(cols).map(([col, value]) => ({ column: columnNames[Number(col)], value }));
    changes.push({ kind: "update", key: keyOf(row), values });
    targets.push({ kind: "update", row });
  }
  edits.inserted.forEach((inserted, index) => {
    const values: ColumnValue[] = [];
    inserted.values.forEach((value, col) => {
      if (value !== DEFAULT) values.push({ column: columnNames[col], value });
    });
    changes.push({ kind: "insert", values });
    targets.push({ kind: "insert", index });
  });
  return { changes, targets };
}

/**
 * The edits still pending once `submitted` has been written: whatever was
 * changed while the submit ran. Loaded rows shift the way `commitRows`
 * shifts them (submitted deletions removed, submitted inserts appended to
 * the loaded rows), so row indexes are remapped. Edits made meanwhile to a
 * row that was just inserted are not kept: that row is loaded data now.
 */
export function afterSubmit(current: PendingEdits, submitted: PendingEdits, rowCount: number): PendingEdits {
  if (current === submitted) return NO_EDITS;
  const removed = [...submitted.deleted].filter((row) => row < rowCount).sort((a, b) => a - b);
  const remap = (row: number) => row - removed.filter((r) => r < row).length;

  const updates: Record<number, Readonly<Record<number, Value>>> = {};
  for (const [rowKey, cols] of Object.entries(current.updates)) {
    const row = Number(rowKey);
    if (submitted.deleted.has(row)) continue;
    const written = submitted.updates[row] ?? {};
    const left = Object.fromEntries(
      Object.entries(cols).filter(([col, value]) => !(col in written && editText(written[Number(col)]) === editText(value))),
    );
    if (Object.keys(left).length > 0) updates[remap(row)] = left;
  }
  const deleted = new Set([...current.deleted].filter((row) => !submitted.deleted.has(row)).map(remap));
  return { updates, deleted, inserted: current.inserted.slice(submitted.inserted.length) };
}

/**
 * Folds submitted changes into the loaded rows, in place: updated rows take
 * what the database returned, inserted rows are appended, deleted rows are
 * removed. Without a returned row, the edited values are used as typed.
 */
export function commitRows(
  rows: Value[][],
  edits: PendingEdits,
  targets: readonly ChangeTarget[],
  returned: readonly (Row | null)[],
): void {
  const rowCount = rows.length;
  targets.forEach((target, i) => {
    const stored = returned[i];
    if (target.kind === "update") {
      rows[target.row] =
        stored ??
        rows[target.row].map((value, col) => (col in edits.updates[target.row] ? edits.updates[target.row][col] : value));
    } else if (target.kind === "insert") {
      rows.push(stored ?? edits.inserted[target.index].values.map((v) => (v === DEFAULT ? null : v)));
    }
  });
  const deleted = targets
    .filter((t): t is { kind: "delete"; row: number } => t.kind === "delete")
    .map((t) => t.row)
    .filter((row) => row < rowCount)
    .sort((a, b) => b - a);
  for (const row of deleted) rows.splice(row, 1);
}
