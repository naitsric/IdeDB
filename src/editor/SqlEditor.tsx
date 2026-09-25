import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { lintKeymap } from "@codemirror/lint";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState } from "@codemirror/state";
import {
  crosshairCursor,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { useEffect, useRef } from "react";
import type { Engine } from "../db/api";
import { renderCompletionIcon, sqlLanguageSupport, type CompletionCatalog } from "./completion";
import { sqlDiagnostics, type LintBackend } from "./diagnostics";
import { editorApi } from "./editorApi";
import { currentStatementHighlight, flashField, intellijKeymap, statementsField } from "./extensions";
import { registerEditor } from "./registry";
import { editorHighlighting, editorTheme } from "./theme";

interface SqlEditorProps {
  consoleId: string;
  engine: Engine;
  /** Read once, when the editor mounts; from then on the editor owns the text. */
  initialValue: string;
  onChange: (sql: string) => void;
  /** Live view of the data source's schemas for completion; read at each request. */
  catalog: CompletionCatalog;
  /** Where live diagnostics are checked; read at each check. */
  lint: LintBackend;
}

/** The console's SQL editor: CodeMirror 6 with dialect-aware highlighting and completion. */
export function SqlEditor({ consoleId, engine, initialValue, onChange, catalog, lint }: SqlEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const language = useRef(new Compartment());
  const latest = useRef({ initialValue, onChange, catalog, lint });
  latest.current = { initialValue, onChange, catalog, lint };

  useEffect(() => {
    const statements = statementsField(engine);
    const liveLint: LintBackend = {
      context: () => latest.current.lint.context(),
      check: (sql) => latest.current.lint.check(sql),
    };

    const editor = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: latest.current.initialValue,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightActiveLine(),
          history(),
          drawSelection(),
          dropCursor(),
          EditorState.allowMultipleSelections.of(true),
          indentOnInput(),
          bracketMatching(),
          closeBrackets(),
          autocompletion({
            icons: false,
            addToOptions: [{ render: renderCompletionIcon, position: 20 }],
          }),
          rectangularSelection(),
          crosshairCursor(),
          highlightSelectionMatches(),
          statements,
          currentStatementHighlight(statements),
          flashField,
          sqlDiagnostics(statements, engine, liveLint),
          language.current.of(sqlLanguageSupport(engine, latest.current.catalog)),
          editorTheme,
          editorHighlighting,
          keymap.of([
            ...intellijKeymap,
            ...closeBracketsKeymap,
            // ⌘⏎ belongs to the app's Execute command, even while it is disabled.
            ...defaultKeymap.filter((binding) => binding.key !== "Mod-Enter"),
            ...searchKeymap,
            ...historyKeymap,
            ...completionKeymap,
            ...lintKeymap,
            indentWithTab,
          ]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current.onChange(update.state.doc.toString());
          }),
          EditorView.contentAttributes.of({
            "aria-label": "SQL console",
            spellcheck: "false",
            autocorrect: "off",
            autocapitalize: "off",
          }),
        ],
      }),
    });
    view.current = editor;
    const unregister = registerEditor(consoleId, editorApi(editor, engine, statements));
    return () => {
      unregister();
      editor.destroy();
      view.current = null;
    };
  }, [consoleId, engine]);

  // Completion reads the catalog live, so only a different data source needs a reconfigure.
  useEffect(() => {
    view.current?.dispatch({ effects: language.current.reconfigure(sqlLanguageSupport(engine, catalog)) });
  }, [engine, catalog]);

  return <div ref={host} data-focus-context="editor" className="size-full overflow-hidden" />;
}
