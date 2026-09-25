import "@glideapps/glide-data-grid/dist/index.css";
import {
  DataEditor,
  GridCellKind,
  GridColumnIcon,
  type DataEditorProps,
  type DataEditorRef,
  type EditListItem,
  type GridCell,
  type GridColumn,
  type GridSelection,
  type Item,
  type Theme,
} from "@glideapps/glide-data-grid";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Column, Value } from "../db/api";
import { token, useTheme } from "../theme";
import { gridRefs } from "./actions";
import { cellValue, DEFAULT, rowState, type CellInput, type PendingEdits } from "./edits";

/**
 * The result-set grid. Wraps Glide Data Grid (canvas, virtualized in both
 * axes) behind our own props, so the grid implementation can be swapped
 * without touching the panels. With `edits` it becomes the data editor:
 * pending changes are painted over the loaded rows and inserted rows follow
 * them.
 */
interface ResultGridProps {
  resultId: number;
  columns: Column[];
  /** Loaded rows; inserted rows come after them. */
  rowCount: number;
  getRow: (index: number) => Value[] | undefined;
  /** Bumped when loaded rows change in place, to repaint. */
  version: number;
  edits?: PendingEdits;
  editable: boolean;
  onCellEdit?: (row: number, col: number, value: Value) => void;
  selection: GridSelection;
  onSelectionChange: (selection: GridSelection) => void;
  onCellContextMenu?: (cell: Item) => void;
}

const GRID_FONT = '12px "JetBrains Mono Variable"';
const CHAR_WIDTH = 7.3;
const MAX_DISPLAY_CHARS = 500;

/**
 * Glide keys we take over: deleting rows, Set NULL and duplicating are
 * commands, and a fill or cut would silently rewrite many cells.
 */
const KEYBINDINGS: DataEditorProps["keybindings"] = { delete: false, clear: false, downFill: false, rightFill: false, cut: false };

export function ResultGrid({
  resultId,
  columns,
  rowCount,
  getRow,
  version,
  edits,
  editable,
  onCellEdit,
  selection,
  onSelectionChange,
  onCellContextMenu,
}: ResultGridProps) {
  const resolvedTheme = useTheme((s) => s.resolved);
  const [fontsReady, setFontsReady] = useState(() => document.fonts.check(GRID_FONT));
  useEffect(() => {
    if (!fontsReady) void document.fonts.load(GRID_FONT).then(() => setFontsReady(true));
  }, [fontsReady]);

  // The canvas cannot read CSS variables, so resolve tokens whenever the theme changes.
  const theme = useMemo(gridTheme, [resolvedTheme, fontsReady]);
  const colors = useMemo(
    () => ({
      muted: token("fg-subtle"),
      modified: token("edit-modified"),
      inserted: token("edit-inserted"),
      deleted: token("edit-deleted"),
      deletedText: token("edit-deleted-fg"),
    }),
    [resolvedTheme],
  );

  const ref = useRef<DataEditorRef>(null);
  useEffect(() => {
    if (ref.current) gridRefs.set(resultId, ref.current);
    return () => void gridRefs.delete(resultId);
  }, [resultId]);

  const [resized, setResized] = useState<Record<string, number>>({});
  useEffect(() => setResized({}), [columns]);

  // Size columns from the header and the first rows, once they arrive.
  const hasRows = rowCount > 0;
  const gridColumns = useMemo<GridColumn[]>(
    () =>
      columns.map((column, i) => ({
        id: String(i),
        title: column.name,
        icon: iconFor(column.typeName),
        width: resized[i] ?? initialWidth(column, i, getRow),
      })),
    [columns, resized, hasRows, getRow],
  );

  const totalRows = rowCount + (edits?.inserted.length ?? 0);

  const getCellContent = useCallback(
    ([col, row]: Item): GridCell => {
      const state = edits ? rowState(edits, rowCount, row) : "clean";
      const { value, edited } = edits
        ? cellValue(edits, rowCount, getRow, row, col)
        : { value: getRow(row)?.[col] ?? null, edited: false };
      const cell = toCell(value, colors.muted, editable && state !== "deleted");
      if (edited && state === "modified") return { ...cell, themeOverride: { ...cell.themeOverride, bgCell: colors.modified } };
      return cell;
    },
    // `version` repaints rows changed in place.
    [getRow, colors, edits, rowCount, editable, version],
  );

  const getRowThemeOverride = useCallback(
    (row: number): Partial<Theme> | undefined => {
      if (!edits) return undefined;
      const state = rowState(edits, rowCount, row);
      if (state === "inserted") return { bgCell: colors.inserted };
      if (state === "deleted") return { bgCell: colors.deleted, textDark: colors.deletedText };
      return undefined;
    },
    [edits, rowCount, colors],
  );

  // Glide routes single edits and pastes alike through here.
  const onCellsEdited = useCallback(
    (items: readonly EditListItem[]) => {
      for (const { location, value } of items) {
        if (value.kind === GridCellKind.Text) onCellEdit?.(location[1], location[0], value.data);
      }
      return true;
    },
    [onCellEdit],
  );

  return (
    <DataEditor
      ref={ref}
      columns={gridColumns}
      rows={totalRows}
      getCellContent={getCellContent}
      getRowThemeOverride={edits ? getRowThemeOverride : undefined}
      theme={theme}
      width="100%"
      height="100%"
      rowHeight={26}
      headerHeight={30}
      rowMarkers={{ kind: "number", width: markerWidth(totalRows) }}
      rangeSelect="multi-rect"
      columnSelect="multi"
      rowSelect="multi"
      gridSelection={selection}
      onGridSelectionChange={onSelectionChange}
      getCellsForSelection
      onCellsEdited={onCellsEdited}
      onPaste={editable}
      keybindings={KEYBINDINGS}
      onCellContextMenu={(cell) => onCellContextMenu?.(cell)}
      smoothScrollX
      smoothScrollY
      onColumnResize={(_, width, index) => setResized((r) => ({ ...r, [index]: width }))}
    />
  );
}

function toCell(value: CellInput, mutedColor: string, editable: boolean): GridCell {
  const base = { kind: GridCellKind.Text, allowOverlay: true, readonly: !editable } as const;
  if (value === DEFAULT) {
    return { ...base, data: "", displayData: "<default>", themeOverride: { textDark: mutedColor } };
  }
  if (value === null) {
    return { ...base, data: "", displayData: "<null>", themeOverride: { textDark: mutedColor } };
  }
  if (typeof value === "number" || typeof value === "bigint") {
    const text = String(value);
    return { ...base, data: text, displayData: text, contentAlign: "right" };
  }
  if (typeof value === "boolean") {
    const text = String(value);
    return { ...base, data: text, displayData: text };
  }
  if (value instanceof Uint8Array) {
    const text = hex(value);
    return { ...base, data: text, displayData: text.length > MAX_DISPLAY_CHARS ? `${text.slice(0, MAX_DISPLAY_CHARS)}…` : text };
  }
  const display = value.length > MAX_DISPLAY_CHARS ? `${value.slice(0, MAX_DISPLAY_CHARS)}…` : value;
  return { ...base, data: value, displayData: display.replace(/\r?\n/g, "↵") };
}

function hex(bytes: Uint8Array): string {
  let out = "0x";
  for (const b of bytes) out += b.toString(16).padStart(2, "0").toUpperCase();
  return out;
}

function iconFor(typeName: string): GridColumnIcon {
  const t = typeName.replace(/^_/, "");
  if (typeName.startsWith("_")) return GridColumnIcon.HeaderArray;
  if (/^(int|float|numeric|decimal|oid|money|serial|double|real)/.test(t)) return GridColumnIcon.HeaderNumber;
  if (t === "bool") return GridColumnIcon.HeaderBoolean;
  if (/^(timestamp|date)/.test(t)) return GridColumnIcon.HeaderDate;
  if (/^(time|interval)/.test(t)) return GridColumnIcon.HeaderTime;
  if (/^json/.test(t)) return GridColumnIcon.HeaderCode;
  if (t === "uuid") return GridColumnIcon.HeaderRowID;
  return GridColumnIcon.HeaderString;
}

function initialWidth(column: Column, index: number, getRow: ResultGridProps["getRow"]): number {
  let chars = column.name.length + 3; // room for the type icon
  for (let r = 0; r < 50; r++) {
    const row = getRow(r);
    if (!row) break;
    const v = row[index];
    const len = v === null ? 6 : v instanceof Uint8Array ? v.length * 2 + 2 : String(v).length;
    chars = Math.max(chars, len);
  }
  return Math.round(Math.min(360, Math.max(64, chars * CHAR_WIDTH + 24)));
}

function markerWidth(rowCount: number): number {
  return Math.max(36, String(rowCount).length * 7.5 + 16);
}

function gridTheme(): Partial<Theme> {
  return {
    accentColor: token("accent"),
    accentFg: token("accent-fg"),
    accentLight: token("selection"),
    textDark: token("fg"),
    textMedium: token("fg-muted"),
    textLight: token("fg-subtle"),
    textBubble: token("fg"),
    textHeader: token("fg-muted"),
    textHeaderSelected: token("accent-fg"),
    bgIconHeader: token("fg-subtle"),
    fgIconHeader: token("bg-panel"),
    bgCell: token("bg-panel"),
    bgCellMedium: token("bg-inset"),
    bgHeader: token("bg"),
    bgHeaderHasFocus: token("bg-elevated"),
    bgHeaderHovered: token("bg-elevated"),
    bgBubble: token("bg-elevated"),
    bgBubbleSelected: token("bg-panel"),
    bgSearchResult: token("accent-soft"),
    borderColor: token("border"),
    horizontalBorderColor: token("border"),
    headerBottomBorderColor: token("border-strong"),
    drilldownBorder: token("border-strong"),
    linkColor: token("accent"),
    fontFamily: token("font-code"),
    baseFontStyle: "12px",
    headerFontStyle: "600 12px",
    markerFontStyle: "11px",
    editorFontSize: "12px",
    lineHeight: 1.4,
    cellHorizontalPadding: 10,
    cellVerticalPadding: 4,
    headerIconSize: 16,
    roundingRadius: 4,
  };
}
