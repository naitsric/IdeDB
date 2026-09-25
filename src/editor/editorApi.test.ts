// @vitest-environment happy-dom
import { forEachDiagnostic, type Diagnostic } from "@codemirror/lint";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";
import { editorApi } from "./editorApi";
import { flashField, statementsField } from "./extensions";

let view: EditorView | undefined;
afterEach(() => view?.destroy());

function setup(doc: string, selection?: { anchor: number; head?: number }) {
  const statements = statementsField("postgres");
  view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc, selection, extensions: [statements, flashField] }),
  });
  return { view, api: editorApi(view, "postgres", statements) };
}

function diagnostics(v: EditorView) {
  const found: Diagnostic[] = [];
  forEachDiagnostic(v.state, (d, from, to) => found.push({ ...d, from, to }));
  return found;
}

describe("statementsToRun", () => {
  it("returns the statement under the caret", () => {
    const { api } = setup("select 1;\nselect 2;", { anchor: 12 });
    expect(api.statementsToRun()).toEqual([{ text: "select 2", from: 10, to: 18 }]);
  });

  it("returns every statement in the selection, with document ranges", () => {
    const doc = "select 0; select 1; select 2; select 3";
    const { api } = setup(doc, { anchor: doc.indexOf("select 1"), head: doc.indexOf("select 3") });
    const run = api.statementsToRun();
    expect(run.map((s) => s.text)).toEqual(["select 1", "select 2"]);
    for (const s of run) expect(doc.slice(s.from, s.to)).toBe(s.text);
  });

  it("follows edits", () => {
    const { view: v, api } = setup("select 1", { anchor: 0 });
    v.dispatch({ changes: { from: 0, insert: "select 0;\n" }, selection: EditorSelection.cursor(0) });
    expect(api.statementsToRun().map((s) => s.text)).toEqual(["select 0"]);
  });
});

describe("insertAtCaret", () => {
  it("starts a new line after a non-blank line", () => {
    const { view: v, api } = setup("select 1", { anchor: 3 });
    api.insertAtCaret("select 2");
    expect(v.state.doc.toString()).toBe("select 1\nselect 2");
    expect(v.state.selection.main.head).toBe(v.state.doc.length);
  });

  it("takes the place of a blank line", () => {
    const { view: v, api } = setup("select 1\n   \nselect 3", { anchor: 10 });
    api.insertAtCaret("select 2");
    expect(v.state.doc.toString()).toBe("select 1\nselect 2\nselect 3");
  });
});

describe("showError", () => {
  it("maps a code point offset past astral characters to the error token", () => {
    const doc = "select 1;\nselect '😀' from from t";
    const { view: v, api } = setup(doc, { anchor: doc.length });
    const [statement] = api.statementsToRun();
    // In code points the second `from` starts at 16; in UTF-16 units at 17.
    api.showError(statement, "syntax error at or near \"from\"", 16);
    const [d] = diagnostics(v);
    expect(doc.slice(d.from, d.to)).toBe("from");
    expect(d.from).toBe(doc.lastIndexOf("from"));
    expect(d.severity).toBe("error");
  });

  it("marks the first token when the engine gives no position", () => {
    const { view: v, api } = setup("select 1;\n  delete from t", { anchor: 20 });
    const [statement] = api.statementsToRun();
    api.showError(statement, "denied");
    const [d] = diagnostics(v);
    expect(v.state.sliceDoc(d.from, d.to)).toBe("delete");
  });

  it("clamps a position at the end of the statement to a point", () => {
    const { view: v, api } = setup("select", { anchor: 0 });
    const [statement] = api.statementsToRun();
    api.showError(statement, "syntax error at end of input", 6);
    const [d] = diagnostics(v);
    expect([d.from, d.to]).toEqual([6, 6]);
    api.clearErrors();
    expect(diagnostics(v)).toEqual([]);
  });
});
