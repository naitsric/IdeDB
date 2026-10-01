import { MySQL, PostgreSQL, SQLite, StandardSQL, type SQLDialect } from "@codemirror/lang-sql";
import { EditorState } from "@codemirror/state";
import { drawSelection, EditorView, lineNumbers } from "@codemirror/view";
import { useLayoutEffect, useRef } from "react";
import type { Engine } from "../db/api";
import { editorHighlighting, editorTheme } from "./theme";

const DIALECT: Record<Engine, SQLDialect> = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite };

/** The console's look, on the inset background, wrapping long lines and growing up to `maxHeight`. */
const viewerTheme = (maxHeight: number) =>
  EditorView.theme({
    "&": { height: "auto", maxHeight: `${maxHeight}px`, backgroundColor: "var(--bg-inset)", fontSize: "12.5px" },
    ".cm-scroller": { overflow: "auto", lineHeight: "1.55" },
    ".cm-content": { padding: "6px 0" },
    ".cm-gutters": { backgroundColor: "var(--bg-inset)" },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 8px 0 10px", minWidth: "28px" },
  });

/**
 * Read-only SQL with the console's highlighting, for showing a statement
 * someone else wrote. Long lines wrap: nothing hides past the right edge.
 * Text can be selected and copied.
 */
export function SqlViewer({
  sql,
  engine,
  maxHeight = 260,
  label,
}: {
  sql: string;
  /** The dialect to highlight; generic SQL when unknown. */
  engine: Engine | undefined;
  maxHeight?: number;
  label: string;
}) {
  const host = useRef<HTMLDivElement>(null);

  // Before paint, so a dialog showing it opens at its final size.
  useLayoutEffect(() => {
    const view = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: sql,
        extensions: [
          lineNumbers(),
          drawSelection(),
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          EditorView.lineWrapping,
          (engine ? DIALECT[engine] : StandardSQL).language,
          // Before the console's theme, so its rules win.
          viewerTheme(maxHeight),
          editorTheme,
          editorHighlighting,
          EditorView.contentAttributes.of({ "aria-label": label, tabindex: "0" }),
        ],
      }),
    });
    return () => view.destroy();
  }, [sql, engine, maxHeight, label]);

  return <div ref={host} className="overflow-hidden rounded-md border border-border" />;
}
