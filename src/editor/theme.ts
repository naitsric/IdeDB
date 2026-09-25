import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

/**
 * Editor chrome and syntax colors, all from design tokens in app.css, so
 * switching light/dark needs no editor reconfiguration.
 */
export const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--bg-panel)",
    color: "var(--fg)",
    fontSize: "13px",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font-code)", lineHeight: "1.6" },
  ".cm-content": { caretColor: "var(--accent)", padding: "8px 0" },
  ".cm-line": { padding: "0 12px 0 4px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
    { backgroundColor: "var(--selection)" },
  ".cm-gutters": { backgroundColor: "var(--bg-panel)", color: "var(--fg-subtle)", border: "none" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 8px 0 14px", minWidth: "36px" },
  ".cm-activeLine": { backgroundColor: "var(--editor-active-line)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--fg-muted)" },
  ".cm-currentStatement": { backgroundColor: "var(--editor-statement)" },
  ".cm-currentStatement.cm-activeLine": { backgroundColor: "var(--editor-statement)" },
  ".cm-flash": { animation: "idedb-flash 700ms ease-out" },
  "&.cm-focused .cm-matchingBracket": { backgroundColor: "var(--editor-bracket)", outline: "none" },
  "&.cm-focused .cm-nonmatchingBracket": { backgroundColor: "transparent", color: "var(--danger)" },
  ".cm-searchMatch": { backgroundColor: "var(--editor-match)", outline: "none" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--editor-flash)" },
  ".cm-selectionMatch": { backgroundColor: "var(--editor-bracket)" },
  ".cm-lintRange-error": {
    backgroundImage: "none",
    textDecoration: "underline wavy var(--danger)",
    textUnderlineOffset: "3px",
    textDecorationSkipInk: "none",
  },

  // Search panel (⌘F)
  ".cm-panels": { backgroundColor: "var(--bg)", color: "var(--fg)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-panel.cm-search": { padding: "6px 8px", fontFamily: "var(--font-ui)", fontSize: "12px" },
  ".cm-panel.cm-search input, .cm-panel.cm-search button, .cm-panel.cm-search label": { fontSize: "12px" },
  ".cm-textfield": {
    backgroundColor: "var(--bg-inset)",
    border: "1px solid var(--border)",
    borderRadius: "var(--radius-sm)",
    color: "var(--fg)",
    padding: "2px 6px",
  },
  ".cm-textfield:focus": { borderColor: "var(--accent)", outline: "none" },
  ".cm-button": {
    backgroundImage: "none",
    backgroundColor: "var(--bg-elevated)",
    border: "1px solid var(--border-strong)",
    borderRadius: "var(--radius-sm)",
    color: "var(--fg)",
  },
  ".cm-panel.cm-search [name=close]": { color: "var(--fg-muted)" },

  // Tooltips: completion and lint messages
  ".cm-tooltip": {
    backgroundColor: "var(--bg-elevated)",
    border: "1px solid var(--border-strong)",
    borderRadius: "var(--radius-md)",
    boxShadow: "var(--shadow-popover)",
    color: "var(--fg)",
    overflow: "hidden",
  },
  ".cm-tooltip-autocomplete > ul": { fontFamily: "var(--font-code)", fontSize: "12.5px", maxHeight: "18em" },
  ".cm-tooltip-autocomplete > ul > li": { padding: "2px 8px" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--accent)", color: "var(--accent-fg)" },
  ".cm-completionDetail": { color: "var(--fg-subtle)", fontStyle: "normal", marginLeft: "1em" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected] .cm-completionDetail": { color: "inherit", opacity: "0.75" },
  ".cm-completionMatchedText": { textDecoration: "none", fontWeight: "600" },
  // Option icons (see renderCompletionIcon), colored like the explorer's.
  ".idedb-completion-icon": {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: "14px",
    marginRight: "6px",
    verticalAlign: "-2px",
    color: "var(--fg-subtle)",
    fontFamily: "var(--font-ui)",
    fontSize: "10px",
    fontWeight: "600",
  },
  ".idedb-completion-icon-table": { color: "#5b8def" },
  ".idedb-completion-icon-view": { color: "#8e7cc3" },
  ".idedb-completion-icon-key": { color: "#d4a72c" },
  ".idedb-completion-icon-keyword": { color: "var(--syntax-keyword)" },
  ".idedb-completion-icon-type": { color: "var(--syntax-type)" },
  ".idedb-completion-icon-variable": { color: "var(--syntax-function)" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected] .idedb-completion-icon": { color: "inherit" },
  ".cm-diagnostic": { fontFamily: "var(--font-ui)", fontSize: "12px", padding: "4px 8px" },
  ".cm-diagnostic-error": { borderLeft: "3px solid var(--danger)" },
});

export const editorHighlighting = syntaxHighlighting(
  HighlightStyle.define([
    { tag: t.keyword, color: "var(--syntax-keyword)" },
    { tag: [t.string, t.special(t.name)], color: "var(--syntax-string)" },
    { tag: t.special(t.string), color: "var(--syntax-identifier)" },
    { tag: [t.number, t.bool, t.null], color: "var(--syntax-number)" },
    { tag: [t.lineComment, t.blockComment], color: "var(--syntax-comment)", fontStyle: "italic" },
    { tag: t.standard(t.name), color: "var(--syntax-function)" },
    { tag: t.typeName, color: "var(--syntax-type)" },
    { tag: [t.operator, t.punctuation, t.paren, t.brace, t.squareBracket], color: "var(--syntax-operator)" },
  ]),
);
