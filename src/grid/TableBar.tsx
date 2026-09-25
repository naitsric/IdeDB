import { ask } from "@tauri-apps/plugin-dialog";
import { Check, Copy, Minus, PanelRight, Plus, Undo2 } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { runTableQuery } from "../actions";
import { executeCommand } from "../commands/registry";
import { useConsoles, type ResultMeta } from "../db/consoles";
import { IconButton, cx } from "../ui/primitives";
import { filterInputs } from "./actions";
import { editsOf, useGrids, type EditorTarget } from "./dataEditor";
import { changeCount, NO_EDITS } from "./edits";

/**
 * The data editor's bar, above a table's rows: WHERE and ORDER BY filters
 * and the editing actions.
 */
export function TableBar({
  consoleId,
  result,
  target,
}: {
  consoleId: string;
  result: ResultMeta;
  target: EditorTarget | undefined;
}) {
  const table = useConsoles((s) => s.consoles[consoleId]?.table);
  const pending = useGrids((s) => changeCount(s.edits[result.id] ?? NO_EDITS));
  const submitting = useGrids((s) => !!s.submitting[result.id]);
  const viewer = useGrids((s) => s.valueViewer);
  const editable = target?.editable === true;

  /** Reruns the table query with the filters, after confirming pending edits may be dropped. */
  const apply = async (filter: { where?: string; orderBy?: string }) => {
    if (changeCount(editsOf(result.id)) > 0) {
      const discard = await ask("Reloading the rows discards the changes you have not submitted.", {
        title: "Discard pending changes?",
        kind: "warning",
        okLabel: "Discard and reload",
      });
      if (!discard) return false;
    }
    useConsoles.getState().setTableFilter(consoleId, filter);
    void runTableQuery(consoleId);
    return true;
  };

  return (
    <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-2">
      <FilterInput
        label="WHERE"
        placeholder="condition, e.g. status = 'paid'"
        applied={table?.where ?? ""}
        onApply={(where) => apply({ where })}
        inputRef={(el) => (el ? filterInputs.set(consoleId, el) : filterInputs.delete(consoleId))}
        className="flex-[3]"
      />
      <FilterInput
        label="ORDER BY"
        placeholder="e.g. created_at desc"
        applied={table?.orderBy ?? ""}
        onApply={(orderBy) => apply({ orderBy })}
        className="flex-[2]"
      />
      <div className="flex shrink-0 items-center gap-0.5">
        <IconButton label="Add Row" shortcut="$mod+KeyN" disabled={!editable} onClick={() => executeCommand("grid.addRow")}>
          <Plus className="size-4" />
        </IconButton>
        <IconButton label="Duplicate Row" shortcut="$mod+KeyD" disabled={!editable} onClick={() => executeCommand("grid.duplicateRows")}>
          <Copy className="size-3.5" />
        </IconButton>
        <IconButton label="Delete Rows" shortcut="$mod+Backspace" disabled={!editable} onClick={() => executeCommand("grid.deleteRows")}>
          <Minus className="size-4" />
        </IconButton>
        <IconButton label="Revert Selected" shortcut="$mod+Alt+KeyZ" disabled={pending === 0} onClick={() => executeCommand("grid.revertSelected")}>
          <Undo2 className="size-3.5" />
        </IconButton>
        <button
          type="button"
          title="Submit changes  ⌘⏎"
          disabled={pending === 0 || submitting}
          onClick={() => executeCommand("grid.submit")}
          className={cx(
            "ml-1 inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12px] font-medium transition-colors",
            pending > 0 ? "bg-accent text-accent-fg hover:brightness-110" : "text-subtle",
            "disabled:pointer-events-none",
            submitting && "opacity-60",
          )}
        >
          <Check className="size-3.5" />
          Submit{pending > 0 && ` (${pending})`}
        </button>
        <IconButton
          label="Value Viewer"
          shortcut="$mod+Alt+KeyV"
          onClick={() => executeCommand("grid.valueViewer")}
          className={cx("ml-1", viewer && "bg-active text-fg")}
        >
          <PanelRight className="size-3.5" />
        </IconButton>
      </div>
      {target && !target.editable && (
        <span className="truncate text-[11.5px] text-subtle" title={target.reason}>
          {target.reason}
        </span>
      )}
    </div>
  );
}

/**
 * One filter clause. Enter applies (empty clears), Esc goes back to the
 * applied text.
 */
function FilterInput({
  label,
  placeholder,
  applied,
  onApply,
  inputRef,
  className,
}: {
  label: string;
  placeholder: string;
  applied: string;
  onApply: (text: string) => Promise<boolean>;
  inputRef?: (el: HTMLInputElement | null) => void;
  className?: string;
}) {
  const [text, setText] = useState(applied);
  const ref = useRef<HTMLInputElement | null>(null);
  useEffect(() => setText(applied), [applied]);

  const onKeyDown = async (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (!(await onApply(text.trim()))) setText(applied);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setText(applied);
      ref.current?.blur();
    }
  };

  return (
    <label
      className={cx(
        "flex h-7 min-w-0 items-center gap-2 rounded-md border bg-inset px-2 focus-within:border-accent",
        text !== applied ? "border-warning" : "border-border",
        className,
      )}
    >
      <span className="shrink-0 text-[10.5px] font-semibold tracking-wide text-subtle">{label}</span>
      <input
        ref={(el) => {
          ref.current = el;
          inputRef?.(el);
        }}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => void onKeyDown(e)}
        placeholder={placeholder}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        className="min-w-0 flex-1 bg-transparent font-mono text-[12px] text-fg outline-none placeholder:text-subtle"
      />
    </label>
  );
}
