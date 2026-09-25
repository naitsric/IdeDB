import type { EditorState } from "@codemirror/state";
import type { Statement } from "./statements";

/**
 * What commands and actions can ask of a console's editor. Each mounted
 * editor registers itself under its console id.
 */
export interface EditorApi {
  /** Every statement in the selection, or the one under the caret without a selection. */
  statementsToRun(): Statement[];
  /** Inserts text on its own line(s) at the caret and selects nothing. */
  insertAtCaret(text: string): void;
  /** Briefly highlights a range, e.g. the statement being executed. */
  flash(range: { from: number; to: number }): void;
  /** Marks an execution error in a statement; `position` is in code points into the statement text. */
  showError(statement: Statement, message: string, position?: number): void;
  clearErrors(): void;
  /** Rechecks the statements on screen, e.g. after the connection or schema changed. */
  refreshDiagnostics(): void;
  /**
   * Formats the selection, or the statement under the caret, as one undo
   * step. `false` when there was nothing to format or it could not be parsed.
   */
  reformat(): boolean;
  /** The editor's current state and caret, e.g. to resolve the name under it. */
  caret(): { state: EditorState; pos: number };
  focus(): void;
}

const editors = new Map<string, EditorApi>();

export function registerEditor(consoleId: string, api: EditorApi): () => void {
  editors.set(consoleId, api);
  return () => {
    if (editors.get(consoleId) === api) editors.delete(consoleId);
  };
}

export function editorFor(consoleId: string): EditorApi | undefined {
  return editors.get(consoleId);
}
