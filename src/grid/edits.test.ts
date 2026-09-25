import { describe, expect, it } from "vitest";
import type { Value } from "../db/api";
import {
  addRow,
  cellValue,
  changeCount,
  commitRows,
  DEFAULT,
  deleteRows,
  duplicateRows,
  NO_EDITS,
  revertRows,
  rowState,
  setCell,
  toChanges,
} from "./edits";

// id (key), name, note
const loaded = (): Value[][] => [
  [1n, "ada", "first"],
  [2, "grace", null],
  [3, "linus", "third"],
];
const columns = ["id", "name", "note"];
const keys = [0];

describe("pending edits", () => {
  it("tracks cell edits and drops them when set back to the original", () => {
    const rows = loaded();
    const get = (i: number) => rows[i];
    let edits = setCell(NO_EDITS, 3, get, 0, 1, "ADA");
    expect(cellValue(edits, 3, get, 0, 1)).toEqual({ value: "ADA", edited: true });
    expect(rowState(edits, 3, 0)).toBe("modified");
    expect(changeCount(edits)).toBe(1);

    edits = setCell(edits, 3, get, 0, 1, "ada");
    expect(rowState(edits, 3, 0)).toBe("clean");
    expect(changeCount(edits)).toBe(0);

    // Retyping a number's text or clearing a NULL is not a change either.
    expect(changeCount(setCell(NO_EDITS, 3, get, 1, 0, "2"))).toBe(0);
    expect(changeCount(setCell(NO_EDITS, 3, get, 1, 2, null))).toBe(0);
  });

  it("adds, duplicates, deletes and reverts rows", () => {
    const rows = loaded();
    const get = (i: number) => rows[i];
    let edits = addRow(NO_EDITS, 3);
    expect(rowState(edits, 3, 3)).toBe("inserted");
    expect(cellValue(edits, 3, get, 3, 0).value).toBe(DEFAULT);

    edits = setCell(edits, 3, get, 3, 1, "new");
    edits = duplicateRows(edits, 3, get, [2], 3, new Set([0]));
    expect(edits.inserted[1].values).toEqual([DEFAULT, "linus", "third"]);

    edits = deleteRows(edits, 3, [1, 4]);
    expect(rowState(edits, 3, 1)).toBe("deleted");
    expect(edits.inserted).toHaveLength(1);
    expect(changeCount(edits)).toBe(2);

    edits = revertRows(edits, 3, [1, 3]);
    expect(changeCount(edits)).toBe(0);
  });

  it("builds deletes, then updates, then inserts, keyed by the original values", () => {
    const rows = loaded();
    const get = (i: number) => rows[i];
    let edits = setCell(NO_EDITS, 3, get, 0, 2, null);
    edits = setCell(edits, 3, get, 0, 0, "10");
    edits = deleteRows(edits, 3, [2]);
    edits = setCell(edits, 3, get, 2, 1, "ignored: row deleted");
    edits = setCell(addRow(edits, 3), 3, get, 3, 1, "new");

    const { changes, targets } = toChanges(edits, get, columns, keys);
    expect(changes).toEqual([
      { kind: "delete", key: [{ column: "id", value: 3 }] },
      {
        kind: "update",
        key: [{ column: "id", value: 1n }],
        values: [
          { column: "id", value: "10" },
          { column: "note", value: null },
        ],
      },
      { kind: "insert", values: [{ column: "name", value: "new" }] },
    ]);
    expect(targets).toEqual([
      { kind: "delete", row: 2 },
      { kind: "update", row: 0 },
      { kind: "insert", index: 0 },
    ]);
  });

  it("commits returned rows in place", () => {
    const rows = loaded();
    const get = (i: number) => rows[i];
    let edits = setCell(NO_EDITS, 3, get, 1, 2, "note");
    edits = setCell(edits, 3, get, 0, 1, "Ada");
    edits = deleteRows(edits, 3, [0]);
    edits = setCell(addRow(edits, 3), 3, get, 3, 1, "new");
    const { targets } = toChanges(edits, get, columns, keys);

    commitRows(rows, edits, targets, [null, [2, "grace", "stored note"], [4, "new", null]]);
    expect(rows).toEqual([
      [2, "grace", "stored note"],
      [3, "linus", "third"],
      [4, "new", null],
    ]);
  });

  it("falls back to the typed values when nothing was read back", () => {
    const rows = loaded();
    const get = (i: number) => rows[i];
    const edits = setCell(addRow(setCell(NO_EDITS, 3, get, 2, 1, "L"), 3), 3, get, 3, 1, "typed");
    const { targets } = toChanges(edits, get, columns, keys);
    commitRows(rows, edits, targets, [null, null]);
    expect(rows[2]).toEqual([3, "L", "third"]);
    expect(rows[3]).toEqual([null, "typed", null]);
  });
});
