import type { StateField } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Engine } from "../db/api";
import { problemRange, refreshDiagnostics, showExecutionError } from "./diagnostics";
import { flashRange } from "./extensions";
import { formatSql } from "./format";
import type { EditorApi } from "./registry";
import { pickStatement, statementsIn, type Statement } from "./statements";

/** The {@link EditorApi} of one editor view. */
export function editorApi(view: EditorView, engine: Engine, statements: StateField<Statement[]>): EditorApi {
  return {
    statementsToRun() {
      const { from, to, empty, head } = view.state.selection.main;
      if (!empty) return statementsIn(view.state.doc.toString(), from, to, engine);
      const statement = pickStatement(view.state.field(statements), head);
      return statement ? [statement] : [];
    },

    insertAtCaret(text) {
      const line = view.state.doc.lineAt(view.state.selection.main.head);
      const blank = line.text.trim() === "";
      // On a blank line, take its place; otherwise start a new line after it.
      const from = blank ? line.from : line.to;
      const insert = blank ? text : `\n${text}`;
      view.dispatch({
        changes: { from, to: blank ? line.to : from, insert },
        selection: { anchor: from + insert.length },
        scrollIntoView: true,
      });
      view.focus();
    },

    flash: (range) => flashRange(view, range),

    showError(statement, message, position) {
      // Clamped inside `problemRange`: the text may have changed while the statement ran.
      showExecutionError(view, { ...problemRange(view.state.doc, statement, position), severity: "error", message });
    },

    clearErrors: () => showExecutionError(view, null),

    refreshDiagnostics: () => refreshDiagnostics(view),

    reformat() {
      const { from, to, empty, head } = view.state.selection.main;
      const range = empty ? pickStatement(view.state.field(statements), head) : { from, to };
      if (!range) return false;
      const original = view.state.sliceDoc(range.from, range.to);
      const formatted = formatSql(original, engine);
      if (formatted === null) return false;
      if (formatted !== original) {
        // One transaction: a single undo step restores the original text.
        view.dispatch({
          changes: { from: range.from, to: range.to, insert: formatted },
          selection: empty ? { anchor: range.from } : { anchor: range.from, head: range.from + formatted.length },
          scrollIntoView: true,
          userEvent: "input.format",
        });
      }
      return true;
    },

    caret: () => ({ state: view.state, pos: view.state.selection.main.head }),

    focus: () => view.focus(),
  };
}
