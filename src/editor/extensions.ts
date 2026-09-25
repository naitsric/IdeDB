import { copyLineDown, deleteLine, moveLineDown, moveLineUp } from "@codemirror/commands";
import { selectNextOccurrence } from "@codemirror/search";
import { StateEffect, StateField, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet, type KeyBinding } from "@codemirror/view";
import type { Engine } from "../db/api";
import { pickStatement, splitStatements, type Statement } from "./statements";

/** The console text split into statements, recomputed only when the text changes. */
export function statementsField(engine: Engine) {
  return StateField.define<Statement[]>({
    create: (state) => splitStatements(state.doc.toString(), engine),
    update: (value, tr) => (tr.docChanged ? splitStatements(tr.newDoc.toString(), engine) : value),
  });
}

const statementLine = Decoration.line({ class: "cm-currentStatement" });

/**
 * Tints the lines of the statement under the caret, as DataGrip does, so it
 * is clear what ⌘⏎ will run. Skipped when the console holds one statement.
 */
export function currentStatementHighlight(field: StateField<Statement[]>): Extension {
  return EditorView.decorations.compute([field, "selection"], (state) => {
    const statements = state.field(field);
    if (statements.length < 2) return Decoration.none;
    const statement = pickStatement(statements, state.selection.main.head);
    if (!statement) return Decoration.none;
    const lines: Range<Decoration>[] = [];
    const last = state.doc.lineAt(statement.to).number;
    for (let n = state.doc.lineAt(statement.from).number; n <= last; n++) {
      lines.push(statementLine.range(state.doc.line(n).from));
    }
    return Decoration.set(lines);
  });
}

const setFlash = StateEffect.define<{ from: number; to: number } | null>();
const flashMark = Decoration.mark({ class: "cm-flash" });

/** Holds the flashed range; the CSS animation does the fading. */
export const flashField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let next = value.map(tr.changes);
    for (const effect of tr.effects) {
      if (!effect.is(setFlash)) continue;
      next = effect.value && effect.value.to > effect.value.from
        ? Decoration.set([flashMark.range(effect.value.from, effect.value.to)])
        : Decoration.none;
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** Matches the `idedb-flash` animation in app.css. */
const FLASH_MS = 700;
const flashTimers = new WeakMap<EditorView, number>();

export function flashRange(view: EditorView, range: { from: number; to: number }) {
  const to = Math.min(range.to, view.state.doc.length);
  // Clear first so flashing the same range twice restarts the animation, and
  // drop the previous timer so it does not cut this flash short.
  window.clearTimeout(flashTimers.get(view));
  view.dispatch({ effects: setFlash.of(null) });
  view.dispatch({ effects: setFlash.of({ from: Math.min(range.from, to), to }) });
  flashTimers.set(
    view,
    window.setTimeout(() => view.dom.isConnected && view.dispatch({ effects: setFlash.of(null) }), FLASH_MS),
  );
}

/**
 * IntelliJ editor keys that differ from CodeMirror's defaults. Listed before
 * the default keymaps so they take precedence.
 */
export const intellijKeymap: readonly KeyBinding[] = [
  { key: "Mod-d", run: copyLineDown, preventDefault: true },
  { key: "Mod-Backspace", run: deleteLine, preventDefault: true },
  { key: "Shift-Alt-ArrowUp", run: moveLineUp, preventDefault: true },
  { key: "Shift-Alt-ArrowDown", run: moveLineDown, preventDefault: true },
  { key: "Ctrl-g", run: selectNextOccurrence, preventDefault: true },
];
