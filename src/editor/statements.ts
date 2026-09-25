import type { Engine } from "../db/api";

/**
 * Splits console text into statements with a lexer rather than a parser, so
 * it works on the incomplete or invalid SQL a console usually holds. It only
 * needs to know where `;` is not a separator: inside quotes, identifiers,
 * comments and Postgres dollar-quoted bodies.
 */

export interface Statement {
  /** The statement without the `;` and without surrounding whitespace or comments. */
  text: string;
  /** Absolute range in the source, UTF-16 offsets, end exclusive. */
  from: number;
  to: number;
}

const isIdentChar = (c: string) => /[\p{L}\p{N}_$]/u.test(c);
const isTagStart = (c: string) => /[\p{L}_]/u.test(c);

/**
 * What the lexer sees: `quoted` covers string literals and quoted
 * identifiers, `dollar` Postgres dollar-quoted bodies, `code` one character
 * of anything else.
 */
export type LexKind = "separator" | "space" | "comment" | "quoted" | "dollar" | "code";

/** Walks `sql` once, reporting every lexical run in order. */
export function scan(sql: string, engine: Engine, visit: (kind: LexKind, from: number, to: number) => void): void {
  let i = 0;
  while (i < sql.length) {
    const start = i;
    const c = sql[i];
    const next = sql[i + 1];

    if (c === ";") {
      visit("separator", start, ++i);
    } else if (/\s/.test(c)) {
      visit("space", start, ++i);
    } else if ((c === "-" && next === "-") || (c === "#" && engine === "mysql")) {
      const end = sql.indexOf("\n", i);
      i = end < 0 ? sql.length : end + 1;
      visit("comment", start, i);
    } else if (c === "/" && next === "*") {
      i = skipBlockComment(sql, i, engine === "postgres");
      visit("comment", start, i);
    } else if (c === "'" || c === '"' || (c === "`" && engine === "mysql")) {
      i = skipQuoted(sql, i, c, backslashEscapes(sql, i, c, engine));
      visit("quoted", start, i);
    } else if (c === "$" && engine === "postgres" && !(i > 0 && isIdentChar(sql[i - 1]))) {
      const tag = dollarTag(sql, i);
      if (tag) {
        const close = sql.indexOf(tag, i + tag.length);
        i = close < 0 ? sql.length : close + tag.length;
        visit("dollar", start, i);
      } else {
        visit("code", start, ++i);
      }
    } else {
      visit("code", start, ++i);
    }
  }
}

export function splitStatements(sql: string, engine: Engine = "postgres"): Statement[] {
  const statements: Statement[] = [];
  // First and last+1 offsets of code (anything but whitespace and comments) in the current statement.
  let codeFrom = -1;
  let codeTo = -1;
  const endStatement = () => {
    if (codeFrom >= 0) statements.push({ text: sql.slice(codeFrom, codeTo), from: codeFrom, to: codeTo });
    codeFrom = codeTo = -1;
  };

  scan(sql, engine, (kind, from, to) => {
    if (kind === "separator") {
      endStatement();
    } else if (kind !== "space" && kind !== "comment") {
      if (codeFrom < 0) codeFrom = from;
      codeTo = to;
    }
  });
  endStatement();
  return statements;
}

/**
 * `sql` with comments, strings, quoted identifiers and dollar-quoted bodies
 * blanked out (same length, newlines kept), for pattern matching on code only.
 */
export function maskNonCode(sql: string, engine: Engine): string {
  let out = "";
  scan(sql, engine, (kind, from, to) => {
    const text = sql.slice(from, to);
    out += kind === "comment" || kind === "quoted" || kind === "dollar" ? text.replace(/[^\n]/g, " ") : text;
  });
  return out;
}

/**
 * The statement to run for a caret: the one containing it (ends inclusive,
 * so a caret right after the last character still counts), else the nearest
 * one before it, else the first.
 */
export function statementAt(sql: string, caret: number, engine?: Engine): Statement | undefined {
  return pickStatement(splitStatements(sql, engine), caret);
}

/** {@link statementAt} over statements already split. */
export function pickStatement(statements: readonly Statement[], caret: number): Statement | undefined {
  let before: Statement | undefined;
  for (const statement of statements) {
    if (statement.from <= caret && caret <= statement.to) return statement;
    if (statement.to <= caret) before = statement;
  }
  return before ?? statements[0];
}

/** Statements within `[from, to)` of `sql`, with ranges absolute to `sql`. */
export function statementsIn(sql: string, from: number, to: number, engine?: Engine): Statement[] {
  return splitStatements(sql.slice(from, to), engine).map((s) => ({ ...s, from: s.from + from, to: s.to + from }));
}

/** Converts an offset in Unicode code points (what the drivers report) to UTF-16 units (what the editor uses). */
export function codePointToUtf16(text: string, codePoints: number): number {
  let units = 0;
  let seen = 0;
  for (const ch of text) {
    if (seen === codePoints) break;
    units += ch.length;
    seen++;
  }
  return units;
}

function skipBlockComment(sql: string, start: number, nests: boolean): number {
  let depth = 0;
  let i = start;
  while (i < sql.length) {
    if (sql[i] === "/" && sql[i + 1] === "*" && (nests || depth === 0)) {
      depth++;
      i += 2;
    } else if (sql[i] === "*" && sql[i + 1] === "/") {
      depth--;
      i += 2;
      if (depth === 0) return i;
    } else {
      i++;
    }
  }
  return sql.length;
}

/** Skips a quoted run; doubled quotes escape, and so does backslash where the dialect allows it. */
function skipQuoted(sql: string, start: number, quote: string, backslash: boolean): number {
  let i = start + 1;
  while (i < sql.length) {
    const c = sql[i];
    if (backslash && c === "\\") {
      i += 2;
    } else if (c === quote) {
      if (sql[i + 1] === quote) i += 2;
      else return i + 1;
    } else {
      i++;
    }
  }
  return sql.length;
}

/** MySQL strings escape with backslash; in Postgres only `E'…'` strings do; SQLite never. */
function backslashEscapes(sql: string, quoteAt: number, quote: string, engine: Engine): boolean {
  if (engine === "mysql") return quote !== "`";
  if (engine === "postgres" && quote === "'") {
    const prefix = sql[quoteAt - 1];
    return (prefix === "E" || prefix === "e") && !(quoteAt > 1 && isIdentChar(sql[quoteAt - 2]));
  }
  return false;
}

/** `$$` or `$tag$` starting at `start`, or undefined for anything else (e.g. `$1`). */
function dollarTag(sql: string, start: number): string | undefined {
  let i = start + 1;
  if (sql[i] === "$") return "$$";
  if (!isTagStart(sql[i] ?? "")) return undefined;
  while (i < sql.length && /[\p{L}\p{N}_]/u.test(sql[i])) i++;
  return sql[i] === "$" ? sql.slice(start, i + 1) : undefined;
}
