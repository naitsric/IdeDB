import { diagnosticCount, forceLinting, linter, setDiagnostics, type Diagnostic } from "@codemirror/lint";
import { StateEffect, StateField, type EditorState, type Extension, type Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Engine, SqlProblem } from "../db/api";
import { codePointToUtf16, maskNonCode, type Statement } from "./statements";

/**
 * Editor diagnostics from two places:
 * - live: shortly after typing stops, the statements on screen are checked
 *   by the engine itself (prepared, never run) on the data source's explorer
 *   session, so what is flagged is exactly what the server would reject;
 * - execution: the error of the last run, until the text is edited.
 */

/** What live checks run against. */
export interface LintBackend {
  /**
   * Identifies everything a check's answer depends on (session, schema,
   * database revision); `null` while checks cannot run (not connected).
   */
  context(): string | null;
  check(sql: string): Promise<SqlProblem | null>;
}

/** Quiet time after the last keystroke before checking. */
const LINT_DELAY_MS = 400;
/** Statements this far outside the viewport are checked too, so scrolling finds them done. */
const VIEWPORT_MARGIN = 2000;
/** Upper bound on statements checked per pass. */
const MAX_CHECKED = 40;
const CACHE_SIZE = 500;

/* ------------------------------------------------------------------------ */
/* Rules                                                                    */
/* ------------------------------------------------------------------------ */

/**
 * Named parameters (`:name`) that the engine cannot prepare, so checking
 * would only report the placeholder. SQLite prepares them fine; `::` casts
 * and `a[1:2]` slices are not parameters.
 */
export function hasUnpreparableParameters(text: string, engine: Engine): boolean {
  if (engine === "sqlite") return false;
  return /(^|[^:\w]):[A-Za-z_]\w*/.test(maskNonCode(text, engine));
}

const CREATE =
  /^create\s+(?:or\s+replace\s+)?(?:(?:global|local)\s+)?(?:(?:temp|temporary|unlogged)\s+)?(?:materialized\s+)?(?:table|view)\s+(?:if\s+not\s+exists\s+)?((?:"[^"]+"|`[^`]+`|[\p{L}\p{N}_$]+)(?:\s*\.\s*(?:"[^"]+"|`[^`]+`|[\p{L}\p{N}_$]+))*)/iu;

/** The table or view a CREATE statement makes, lowercased and without its schema. */
export function createdName(text: string): string | null {
  const path = CREATE.exec(text)?.[1];
  return path ? lastPart(path) : null;
}

/** Tables and views created by the statements before `index`. */
export function createdBefore(statements: readonly Statement[], index: number): Set<string> {
  const names = new Set<string>();
  for (const statement of statements.slice(0, index)) {
    const name = createdName(statement.text);
    if (name) names.add(name);
  }
  return names;
}

/** The table a "does not exist" problem is about, lowercased and without its schema. */
export function missingTable(message: string): string | null {
  const name =
    /relation "([^"]+)" does not exist/.exec(message)?.[1] ?? // Postgres
    /Table '([^']+)' doesn't exist/.exec(message)?.[1] ?? // MySQL
    /no such table: (\S+)/.exec(message)?.[1]; // SQLite
  return name ? lastPart(name) : null;
}

function lastPart(path: string): string {
  const last = path.split(".").at(-1)!.trim();
  return last.replace(/^["`](.*)["`]$/, "$1").toLowerCase();
}

/**
 * Whether a problem is only that the statement uses a table created earlier
 * in the same console, which the server does not know yet because that
 * CREATE has not run.
 */
export function createdInDocument(problem: SqlProblem, statements: readonly Statement[], index: number): boolean {
  const missing = missingTable(problem.message);
  return !!missing && createdBefore(statements, index).has(missing);
}

/** Where a problem is: from its position (code points into the statement) to the end of that token. */
export function problemRange(doc: Text, statement: Statement, position: number | null | undefined) {
  const end = Math.min(statement.to, doc.length);
  const from = Math.min(
    position == null ? statement.from : statement.from + codePointToUtf16(statement.text, position),
    end,
  );
  const word = /^[\p{L}\p{N}_$"`.]+/u.exec(doc.sliceString(from, end))?.[0].length ?? 0;
  return { from, to: from + (word || Math.min(1, end - from)) };
}

/* ------------------------------------------------------------------------ */
/* Extension                                                                */
/* ------------------------------------------------------------------------ */

/** The last execution error, kept until the text changes. */
const setExecutionError = StateEffect.define<Diagnostic | null>();

const executionError = StateField.define<Diagnostic | null>({
  create: () => null,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(setExecutionError)) return effect.value;
    return tr.docChanged ? null : value;
  },
});

/** Asks the live checks to run again (connection or schema changed, database may have). */
const refresh = StateEffect.define<null>();

/**
 * Shows (or clears) the execution error right away, then lets the live
 * checks run again so their diagnostics come back next to it.
 */
export function showExecutionError(view: EditorView, diagnostic: Diagnostic | null) {
  view.dispatch(setDiagnostics(view.state, diagnostic ? [diagnostic] : []));
  view.dispatch({ effects: setExecutionError.of(diagnostic) });
  forceLinting(view);
}

export function refreshDiagnostics(view: EditorView) {
  view.dispatch({ effects: refresh.of(null) });
}

/** Results by context and statement text, so unchanged statements are not rechecked on every edit. */
const cache = new Map<string, Promise<SqlProblem | null>>();

function cachedCheck(backend: LintBackend, context: string, text: string): Promise<SqlProblem | null> {
  const key = `${context}\0${text}`;
  let result = cache.get(key);
  if (!result) {
    result = backend.check(text).catch(() => {
      // A failed call (e.g. the session closed) says nothing about the SQL.
      cache.delete(key);
      return null;
    });
    cache.set(key, result);
    if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
  }
  return result;
}

/** Statements to check: those on screen, give or take a margin. */
function statementsNearViewport(view: EditorView, statements: readonly Statement[]): [Statement, number][] {
  const { from, to } = view.viewport;
  return statements
    .map((s, i) => [s, i] as [Statement, number])
    .filter(([s]) => s.to >= from - VIEWPORT_MARGIN && s.from <= to + VIEWPORT_MARGIN)
    .slice(0, MAX_CHECKED);
}

export function sqlDiagnostics(
  statementsField: StateField<Statement[]>,
  engine: Engine,
  backend: LintBackend,
): Extension {
  return [
    executionError,
    // A marker describes text that no longer exists once edited: clear right
    // away rather than leave it misplaced until the next check.
    EditorView.updateListener.of((update) => {
      if (update.docChanged && diagnosticCount(update.state) > 0) {
        queueMicrotask(() => update.view.dispatch(setDiagnostics(update.view.state, [])));
      }
    }),
    linter(
      async (view) => {
        const state: EditorState = view.state;
        const executed = state.field(executionError);
        const context = backend.context();
        if (!context) return executed ? [executed] : [];

        const statements = state.field(statementsField);
        const checked = await Promise.all(
          statementsNearViewport(view, statements).map(async ([statement, index]) => {
            if (hasUnpreparableParameters(statement.text, engine)) return null;
            const problem = await cachedCheck(backend, context, statement.text);
            if (!problem || createdInDocument(problem, statements, index)) return null;
            return { ...problemRange(state.doc, statement, problem.position), severity: "error", message: problem.message } as Diagnostic;
          }),
        );
        const live = checked.filter((d): d is Diagnostic => !!d);
        // The execution error already marks its statement; do not flag it twice.
        return executed ? [executed, ...live.filter((d) => d.to < executed.from || d.from > executed.to)] : live;
      },
      {
        delay: LINT_DELAY_MS,
        needsRefresh: (update) =>
          update.transactions.some((tr) => tr.effects.some((e) => e.is(refresh) || e.is(setExecutionError))),
      },
    ),
  ];
}
