import { setDiagnostics } from "@codemirror/lint";
import type { StateField } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Engine } from "../db/api";
import { flashRange } from "./extensions";
import type { EditorApi } from "./registry";
import { codePointToUtf16, pickStatement, statementsIn, type Statement } from "./statements";

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
      // Clamped: the text may have changed while the statement ran.
      const end = Math.min(statement.to, view.state.doc.length);
      const from = Math.min(
        position === undefined ? statement.from : statement.from + codePointToUtf16(statement.text, position),
        end,
      );
      const to = tokenEnd(view.state.doc.sliceString(from, end), from);
      view.dispatch(setDiagnostics(view.state, [{ from, to, severity: "error", message }]));
    },

    clearErrors: () => view.dispatch(setDiagnostics(view.state, [])),

    focus: () => view.focus(),
  };
}

/** End of the word starting at `from`, so the error underline covers one token. */
function tokenEnd(rest: string, from: number): number {
  const word = /^[\p{L}\p{N}_$"`.]+/u.exec(rest)?.[0].length ?? 0;
  return from + (word || Math.min(1, rest.length));
}
