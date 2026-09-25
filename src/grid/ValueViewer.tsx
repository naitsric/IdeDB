import { useEffect, useMemo, useState } from "react";
import { rowGetter, type ResultMeta } from "../db/consoles";
import { Button } from "../ui/primitives";
import { editCell } from "./actions";
import { useGrids, type EditorTarget } from "./dataEditor";
import { cellValue, DEFAULT, editText, NO_EDITS, rowState, type CellInput } from "./edits";

/**
 * Side panel showing the selected cell in full: pretty-printed JSON, long
 * text with its line breaks, bytes as a hex dump. Editable cells can be
 * edited here too, which suits JSON and multi-line text better than the
 * in-grid editor.
 */
export function ValueViewer({
  consoleId,
  result,
  target,
}: {
  consoleId: string;
  result: ResultMeta;
  target: EditorTarget | undefined;
}) {
  const cell = useGrids((s) => s.selections[result.id]?.current?.cell);
  const edits = useGrids((s) => s.edits[result.id]) ?? NO_EDITS;
  const [col, row] = cell ?? [-1, -1];
  const column = result.columns[col];

  const value: CellInput | undefined = useMemo(
    () => (column ? cellValue(edits, result.rowCount, rowGetter(result.id), row, col).value : undefined),
    // `version` covers rows changed in place by a submit.
    [column, edits, result.rowCount, result.id, result.version, row, col],
  );
  const editable = target?.editable === true && column !== undefined && rowState(edits, result.rowCount, row) !== "deleted";
  const shown = value === undefined ? "" : display(value, column?.typeName ?? "", editable);
  const [draft, setDraft] = useState(shown);
  useEffect(() => setDraft(shown), [shown]);

  if (!column || value === undefined) {
    return <Placeholder>Select a cell to see its full value.</Placeholder>;
  }

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3 text-[12px]">
        <span className="truncate font-medium text-fg">{column.name}</span>
        <span className="truncate font-mono text-[11px] text-subtle">{column.typeName}</span>
      </div>
      {editable ? (
        <>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            spellCheck={false}
            placeholder={value === null ? "<null>" : value === DEFAULT ? "<default>" : ""}
            aria-label={`Value of ${column.name}`}
            className="min-h-0 flex-1 resize-none bg-panel p-3 font-mono text-[12px] leading-[1.55] text-fg outline-none placeholder:text-subtle"
          />
          <div className="flex shrink-0 items-center justify-end gap-2 border-t border-border px-3 py-2">
            <Button variant="ghost" onClick={() => editCell(consoleId, result.id, row, col, null)}>
              Set NULL
            </Button>
            <Button variant="primary" disabled={draft === shown} onClick={() => editCell(consoleId, result.id, row, col, draft)}>
              Apply
            </Button>
          </div>
        </>
      ) : value === null || value === DEFAULT ? (
        <Placeholder>{value === null ? "NULL" : "Default value"}</Placeholder>
      ) : (
        <pre className="selectable min-h-0 flex-1 overflow-auto p-3 font-mono text-[12px] leading-[1.55] whitespace-pre-wrap text-fg">
          {shown}
        </pre>
      )}
    </div>
  );
}

function Placeholder({ children }: { children: string }) {
  return <div className="flex h-full items-center justify-center bg-panel p-4 text-center text-[12px] text-subtle">{children}</div>;
}

/**
 * How a value reads best in full: JSON indented, bytes as a hex dump (or as
 * `0x…` when editing, which is what an edit accepts), the rest as text.
 */
function display(value: CellInput, typeName: string, forEditing: boolean): string {
  if (value === null || value === DEFAULT) return "";
  if (value instanceof Uint8Array) return forEditing ? (editText(value) ?? "") : hexDump(value);
  const text = String(value);
  if (/json/i.test(typeName) || /^\s*[[{]/.test(text)) {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      // Not JSON after all; show as is.
    }
  }
  return text;
}

function hexDump(bytes: Uint8Array): string {
  const lines: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += 16) {
    const chunk = bytes.subarray(offset, offset + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(chunk, (b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${offset.toString(16).padStart(8, "0")}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return lines.join("\n");
}
