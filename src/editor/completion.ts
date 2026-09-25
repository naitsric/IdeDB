import { startCompletion, type Completion, type CompletionContext, type CompletionResult, type CompletionSource } from "@codemirror/autocomplete";
import { keywordCompletionSource, MySQL, PostgreSQL, SQLite, type SQLDialect } from "@codemirror/lang-sql";
import { LanguageSupport, syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { ColumnInfo, Engine, SchemaInfo, TableInfo } from "../db/api";
import type { SchemaLoad } from "../db/dataSources";
import { quoteIdent } from "../db/sql";

/**
 * Schema-aware SQL completion, DataGrip style:
 * - schema names complete as soon as the data source is connected, before
 *   their tables are introspected;
 * - `schema.` loads that schema on demand and then offers its tables;
 * - in table positions (after FROM, JOIN, UPDATE, INTO, TABLE) schemas and
 *   tables are offered instead of keywords;
 * - `alias.` and `table.` offer the columns of the tables in the statement.
 *
 * The source reads the catalog live at every request, so newly introspected
 * schemas show up without reconfiguring the editor.
 */

/** What completion needs to know about a data source, read at request time. */
export interface CatalogSnapshot {
  engine: Engine;
  /** Where unqualified names resolve (search_path head, current database, `main`). */
  defaultSchema: string | null;
  /** Every schema, system ones included. */
  schemas: SchemaInfo[];
  showSystemSchemas: boolean;
  models: Record<string, SchemaLoad>;
}

export interface CompletionCatalog {
  /** `null` while the data source is not connected. */
  snapshot(): CatalogSnapshot | null;
  /** Resolves once the schema is introspected (or failed). */
  loadSchema(schema: string): Promise<void>;
}

const DIALECT: Record<Engine, SQLDialect> = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite };

/** The dialect's language with our schema completion and position-aware keywords. */
export function sqlLanguageSupport(engine: Engine, catalog: CompletionCatalog): LanguageSupport {
  const dialect = DIALECT[engine];
  return new LanguageSupport(dialect.language, [
    dialect.language.data.of({ autocomplete: schemaCompletionSource(catalog) }),
    dialect.language.data.of({ autocomplete: keywordSource(dialect, catalog) }),
  ]);
}

/** Keywords everywhere except table positions, where they would crowd out the tables. */
function keywordSource(dialect: SQLDialect, catalog: CompletionCatalog): CompletionSource {
  const keywords = keywordCompletionSource(dialect);
  return (context) => {
    const at = analyze(context.state, context.pos);
    // After `x.` only members make sense.
    if (at.parents.length > 0) return null;
    // Without a connection there are no tables to offer, so keywords stay.
    if (at.tablePosition && catalog.snapshot()) return null;
    return keywords(context);
  };
}

const IDENT = /^\w*$/;
const QUOTED_IDENT = /^[`"]?\w*[`"]?$/;

/** Words after which a table name is expected. */
const TABLE_KEYWORDS = new Set(["from", "join", "update", "into", "table"]);

export function schemaCompletionSource(catalog: CompletionCatalog): CompletionSource {
  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    const at = analyze(context.state, context.pos);
    // Right after JOIN or ON the popup opens by itself, with the joins foreign keys suggest.
    const joinSpot = at.clause === "join" || at.afterOn;
    if (at.skip || (at.empty && !context.explicit && !joinSpot)) return null;
    const snapshot = catalog.snapshot();
    if (!snapshot) return null;

    let options: Completion[] | null;
    if (at.afterOn && at.empty) {
      options = onConditionOptions(snapshot, at);
    } else if (at.parents.length === 0) {
      options = topLevel(snapshot, at);
    } else {
      options = await members(snapshot, at, catalog);
      if (context.aborted) return null;
    }
    if (!options || options.length === 0) return null;

    if (at.quote) {
      // The user opened a quoted identifier: complete bare names inside the quotes.
      const close = at.quote;
      const quoteAfter = context.state.sliceDoc(context.pos, context.pos + 1) === close;
      return {
        from: at.from,
        to: quoteAfter ? context.pos + 1 : undefined,
        options: options.map((o) => ({ ...o, label: `${at.quote}${o.label}${close}`, apply: undefined })),
        validFor: QUOTED_IDENT,
      };
    }
    return { from: at.from, options, validFor: IDENT };
  };
}

/* ------------------------------------------------------------------------ */
/* Options                                                                  */
/* ------------------------------------------------------------------------ */

function topLevel(snapshot: CatalogSnapshot, at: Analysis): Completion[] {
  const { engine, defaultSchema } = snapshot;
  const options: Completion[] = [];

  if (at.tablePosition) {
    if (at.clause === "join") options.push(...joinOptions(snapshot, at));
    for (const schema of visibleSchemas(snapshot)) {
      options.push({
        label: schema.name,
        type: "schema",
        detail: "schema",
        boost: 1,
        // Inserting a schema goes straight on to its tables, as in DataGrip.
        apply: (view, _completion, from, to) => {
          const text = `${quoteIdent(engine, schema.name)}.`;
          view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length } });
          startCompletion(view);
        },
      });
    }
    for (const [schema, tables] of loadedSchemas(snapshot)) {
      const isDefault = schema === defaultSchema;
      for (const table of tables) {
        options.push({
          ...tableCompletion(engine, table),
          detail: isDefault ? kindLabel(table) : schema,
          apply: isDefault ? quoteIdentApply(engine, table.name) : `${quoteIdent(engine, schema)}.${quoteIdent(engine, table.name)}`,
          boost: isDefault ? 3 : 0,
        });
      }
    }
    return options;
  }

  // Elsewhere: join conditions right after ON, columns of the statement's
  // tables, then aliases, then what lang-sql would offer at the top level
  // (schemas and default-schema tables).
  if (at.afterOn) options.push(...onConditionOptions(snapshot, at));
  const seenTables = new Set<TableInfo>();
  for (const ref of at.refs) {
    const resolved = resolveTable(snapshot, ref.path);
    if (!resolved || resolved === "unloaded" || seenTables.has(resolved.table)) continue;
    seenTables.add(resolved.table);
    for (const column of resolved.table.columns) {
      options.push({ ...columnCompletion(engine, column), detail: `${column.typeName} · ${ref.alias ?? resolved.table.name}`, boost: 2 });
    }
  }
  for (const ref of at.refs) {
    if (ref.alias) options.push({ label: ref.alias, type: "alias", detail: ref.path.join("."), boost: 1 });
  }
  for (const schema of visibleSchemas(snapshot)) {
    options.push({ label: schema.name, type: "schema", detail: "schema", apply: quoteIdentApply(engine, schema.name) });
  }
  const defaultTables = defaultSchema ? loadedModel(snapshot, defaultSchema) : null;
  for (const table of defaultTables ?? []) options.push(tableCompletion(engine, table));
  return options;
}

/* ------------------------------------------------------------------------ */
/* Joins from foreign keys                                                  */
/* ------------------------------------------------------------------------ */

/** A table the statement already names, resolved in the catalog. */
interface JoinedTable {
  schema: string;
  table: TableInfo;
  /** How conditions refer to it: its alias, else its name. */
  ref: string;
}

/** A foreign key between two tables: `from.columns` reference `to.columns`. */
interface Link {
  from: { schema: string; table: TableInfo; columns: string[] };
  to: { schema: string; table: TableInfo; columns: string[] };
}

function joinedTables(snapshot: CatalogSnapshot, refs: readonly TableRef[]): JoinedTable[] {
  const joined: JoinedTable[] = [];
  for (const ref of refs) {
    const resolved = resolveTable(snapshot, ref.path);
    if (!resolved || resolved === "unloaded") continue;
    joined.push({ ...resolved, ref: ref.alias ?? quoteIdent(snapshot.engine, resolved.table.name) });
  }
  return joined;
}

/** Every foreign key from or to `table`, among the loaded schemas. */
function linksOf(snapshot: CatalogSnapshot, schema: string, table: TableInfo): Link[] {
  const links: Link[] = [];
  for (const fk of table.foreignKeys) {
    const target = findTable(loadedModel(snapshot, fk.referencedSchema) ?? [], fk.referencedTable);
    if (target) {
      links.push({
        from: { schema, table, columns: fk.columns },
        to: { schema: fk.referencedSchema, table: target, columns: fk.referencedColumns },
      });
    }
  }
  for (const [otherSchema, tables] of loadedSchemas(snapshot)) {
    for (const other of tables) {
      for (const fk of other.foreignKeys) {
        if (fk.referencedSchema !== schema || fk.referencedTable !== table.name) continue;
        // A self-reference was already found above, from the other side.
        if (other === table) continue;
        links.push({
          from: { schema: otherSchema, table: other, columns: fk.columns },
          to: { schema, table, columns: fk.referencedColumns },
        });
      }
    }
  }
  return links;
}

/** `a.x = b.y and a.z = b.w`, pairing columns by position. */
function condition(engine: Engine, leftRef: string, left: string[], rightRef: string, right: string[]): string {
  return left
    .map((column, i) => `${leftRef}.${quoteIdent(engine, column)} = ${rightRef}.${quoteIdent(engine, right[i])}`)
    .join(" and ");
}

/** Words reserved in join syntax, which cannot be an alias. */
const NOT_ALIASES = new Set(["as", "at", "by", "do", "if", "in", "is", "of", "on", "or", "to"]);

/** DataGrip-style alias: the initials of the name's words (`order_items` → `oi`), made unique. */
export function aliasFor(name: string, taken: ReadonlySet<string>): string {
  const words = name.split(/[_\s-]+|(?<=[a-z])(?=[A-Z])/).filter(Boolean);
  let base = words.map((w) => w[0]).join("").toLowerCase().replace(/[^a-z0-9_]/g, "");
  if (!/^[a-z_]/.test(base)) base = `t${base}`;
  if (!taken.has(base) && !NOT_ALIASES.has(base)) return base;
  for (let n = 1; ; n++) if (!taken.has(`${base}${n}`)) return `${base}${n}`;
}

/**
 * After JOIN: every table linked by a foreign key to a table already in the
 * statement, as a full `table alias on alias.col = other.col` snippet.
 */
function joinOptions(snapshot: CatalogSnapshot, at: Analysis): Completion[] {
  const { engine, defaultSchema } = snapshot;
  // The word being typed after JOIN is not a table of the statement yet.
  const joined = joinedTables(snapshot, at.refs.filter((r) => r.from < at.from));
  const taken = new Set(joined.map((j) => j.ref.toLowerCase()));
  const options: Completion[] = [];
  const seen = new Set<string>();

  for (const existing of joined) {
    for (const link of linksOf(snapshot, existing.schema, existing.table)) {
      // The side of the link that is not the existing table is the one to join.
      const outgoing = link.from.table === existing.table;
      const target = outgoing ? link.to : link.from;
      const alias = aliasFor(target.table.name, taken);
      const on = outgoing
        ? condition(engine, alias, link.to.columns, existing.ref, link.from.columns)
        : condition(engine, alias, link.from.columns, existing.ref, link.to.columns);
      const name =
        target.schema === defaultSchema
          ? quoteIdent(engine, target.table.name)
          : `${quoteIdent(engine, target.schema)}.${quoteIdent(engine, target.table.name)}`;
      const label = `${target.table.name} ${alias} on ${on}`;
      if (seen.has(label)) continue;
      seen.add(label);
      options.push({ label, apply: `${name} ${alias} on ${on}`, type: "join", detail: "foreign key", boost: 5 });
    }
  }
  return options;
}

/** After ON: the foreign key conditions between the table just joined and the ones before it. */
function onConditionOptions(snapshot: CatalogSnapshot, at: Analysis): Completion[] {
  const { engine } = snapshot;
  const joined = joinedTables(snapshot, at.refs.filter((r) => r.from < at.from));
  const last = joined.at(-1);
  if (!last) return [];
  const options: Completion[] = [];
  for (const link of linksOf(snapshot, last.schema, last.table)) {
    const outgoing = link.from.table === last.table;
    const otherTable = outgoing ? link.to.table : link.from.table;
    for (const other of joined.slice(0, -1).filter((j) => j.table === otherTable)) {
      const label = outgoing
        ? condition(engine, last.ref, link.from.columns, other.ref, link.to.columns)
        : condition(engine, last.ref, link.to.columns, other.ref, link.from.columns);
      options.push({ label, type: "join", detail: "foreign key", boost: 5 });
    }
  }
  return options;
}

/** Completions after `a.` or `a.b.`: tables of a schema, or columns of a table or alias. */
async function members(snapshot: CatalogSnapshot, at: Analysis, catalog: CompletionCatalog): Promise<Completion[] | null> {
  const { engine } = snapshot;
  const [first, second] = at.parents;

  if (at.parents.length === 1) {
    // An alias or a table named in this statement.
    const ref = at.refs.find((r) => r.alias === first) ?? at.refs.find((r) => !r.alias && r.path.at(-1) === first);
    const tablePath = ref?.path ?? [first];
    let resolved = resolveTable(snapshot, tablePath);
    if (resolved === "unloaded") resolved = await loadThenResolve(catalog, tablePath);
    if (resolved) return resolved.table.columns.map((c) => columnCompletion(engine, c));

    // A schema: its tables, loading it first if needed.
    const schema = findSchema(snapshot, first);
    if (!schema) return null;
    let tables = loadedModel(snapshot, schema);
    if (!tables) {
      await catalog.loadSchema(schema);
      const fresh = catalog.snapshot();
      tables = fresh ? loadedModel(fresh, schema) : null;
    }
    return tables?.map((t) => tableCompletion(engine, t)) ?? null;
  }

  if (at.parents.length === 2) {
    let resolved = resolveTable(snapshot, [first, second]);
    if (resolved === "unloaded") resolved = await loadThenResolve(catalog, [first, second]);
    return resolved ? resolved.table.columns.map((c) => columnCompletion(engine, c)) : null;
  }

  return null;
}

async function loadThenResolve(catalog: CompletionCatalog, path: string[]) {
  const schema = path.length === 2 ? findSchema(catalog.snapshot(), path[0]) : null;
  if (!schema) return null;
  await catalog.loadSchema(schema);
  const fresh = catalog.snapshot();
  const resolved = fresh ? resolveTable(fresh, path) : null;
  return resolved === "unloaded" ? null : resolved;
}

function tableCompletion(engine: Engine, table: TableInfo): Completion {
  const view = table.kind === "view" || table.kind === "materializedView";
  return {
    label: table.name,
    type: view ? "view" : "table",
    detail: kindLabel(table),
    info: table.comment ?? undefined,
    apply: quoteIdentApply(engine, table.name),
  };
}

function columnCompletion(engine: Engine, column: ColumnInfo): Completion {
  return {
    label: column.name,
    type: column.primaryKey ? "column key" : "column",
    detail: column.typeName,
    info: column.comment ?? undefined,
    apply: quoteIdentApply(engine, column.name),
  };
}

function kindLabel(table: TableInfo): string {
  return table.kind === "view" || table.kind === "materializedView" ? "view" : "table";
}

/** Only set `apply` when the name needs quoting, so plain names insert as typed-matched. */
function quoteIdentApply(engine: Engine, name: string): string | undefined {
  const quoted = quoteIdent(engine, name);
  return quoted === name ? undefined : quoted;
}

/* ------------------------------------------------------------------------ */
/* Catalog lookups                                                          */
/* ------------------------------------------------------------------------ */

function visibleSchemas({ schemas, showSystemSchemas, defaultSchema }: CatalogSnapshot): SchemaInfo[] {
  return schemas.filter((s) => showSystemSchemas || !s.isSystem || s.name === defaultSchema);
}

function* loadedSchemas(snapshot: CatalogSnapshot): Generator<[string, TableInfo[]]> {
  for (const schema of visibleSchemas(snapshot)) {
    const tables = loadedModel(snapshot, schema.name);
    if (tables) yield [schema.name, tables];
  }
}

function loadedModel(snapshot: CatalogSnapshot, schema: string): TableInfo[] | null {
  const load = snapshot.models[schema];
  return load?.state === "loaded" ? load.model.tables : null;
}

/** Exact name first, then case-insensitively (unquoted identifiers fold case). */
export function findSchema(snapshot: CatalogSnapshot | null, name: string): string | null {
  if (!snapshot) return null;
  const exact = snapshot.schemas.find((s) => s.name === name);
  if (exact) return exact.name;
  return snapshot.schemas.find((s) => s.name.toLowerCase() === name.toLowerCase())?.name ?? null;
}

function findTable(tables: TableInfo[], name: string): TableInfo | undefined {
  return tables.find((t) => t.name === name) ?? tables.find((t) => t.name.toLowerCase() === name.toLowerCase());
}

/**
 * `[table]` resolves in the default schema, then in any loaded schema;
 * `[schema, table]` in that schema. "unloaded" means the schema exists but
 * is not introspected yet.
 */
export function resolveTable(
  snapshot: CatalogSnapshot,
  path: string[],
): { schema: string; table: TableInfo } | "unloaded" | null {
  if (path.length === 2) {
    const schema = findSchema(snapshot, path[0]);
    if (!schema) return null;
    const tables = loadedModel(snapshot, schema);
    if (!tables) return snapshot.models[schema]?.state === "error" ? null : "unloaded";
    const table = findTable(tables, path[1]);
    return table ? { schema, table } : null;
  }
  if (path.length !== 1) return null;
  const order = [snapshot.defaultSchema, ...Object.keys(snapshot.models)].filter((s): s is string => !!s);
  for (const schema of new Set(order)) {
    const tables = loadedModel(snapshot, schema);
    const table = tables && findTable(tables, path[0]);
    if (table) return { schema, table };
  }
  return null;
}

/* ------------------------------------------------------------------------ */
/* Syntax analysis                                                          */
/* ------------------------------------------------------------------------ */

/** Lezer's node type, without depending on @lezer/common directly. */
export type SyntaxNode = ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]>;

export interface TableRef {
  path: string[];
  alias?: string;
  /** Where the reference starts in the document. */
  from: number;
}

export interface Analysis {
  /** Start of the word being completed. */
  from: number;
  /** Opening quote when completing inside a quoted identifier. */
  quote: string | null;
  /** Qualifiers before the word: `shop.or|` → ["shop"]. */
  parents: string[];
  /** Nothing typed yet (only explicit completion should open). */
  empty: boolean;
  /** Inside a string or comment: no completion. */
  skip: boolean;
  /** A table name is expected here. */
  tablePosition: boolean;
  /** The keyword that makes this a table position (`from`, `join`, …). */
  clause: string | null;
  /** Right after `ON`, where a join condition starts. */
  afterOn: boolean;
  /** Tables the current statement reads or writes, with their aliases. */
  refs: TableRef[];
}

export const isId = (node: SyntaxNode | null) => node?.name === "Identifier" || node?.name === "QuotedIdentifier";

export function idName(state: EditorState, node: SyntaxNode): string {
  const text = state.sliceDoc(node.from, node.to);
  const quoted = /^([`"])(.*)\1$/.exec(text);
  return quoted ? quoted[2] : text;
}

function pathOf(state: EditorState, node: SyntaxNode): string[] {
  if (node.name !== "CompositeIdentifier") return [idName(state, node)];
  const path: string[] = [];
  for (let child = node.firstChild; child; child = child.nextSibling) if (isId(child)) path.push(idName(state, child));
  return path;
}

function isTrivia(node: SyntaxNode) {
  return /Comment/.test(node.name) || node.name === "⚠";
}

export function previousToken(node: SyntaxNode): SyntaxNode | null {
  let prev = node.prevSibling;
  while (prev && isTrivia(prev)) prev = prev.prevSibling;
  return prev;
}

export function analyze(state: EditorState, pos: number): Analysis {
  const tree = syntaxTree(state);
  let node = tree.resolveInner(pos, -1);
  if (node.name === "⚠") node = node.prevSibling ?? node.parent ?? node;

  const base = {
    quote: null,
    parents: [] as string[],
    empty: false,
    skip: false,
    tablePosition: false,
    clause: null,
    afterOn: false,
    refs: [] as TableRef[],
  };
  if (/Comment|String/.test(node.name)) return { ...base, from: pos, skip: true };

  // The word being typed, if any, and the node the qualified path starts at.
  let from = pos;
  let quote: string | null = null;
  let word: SyntaxNode | null = null;
  if ((isId(node) || node.name === "Keyword") && node.to >= pos) {
    word = node;
    from = node.from;
    if (node.name === "QuotedIdentifier") quote = state.sliceDoc(node.from, node.from + 1);
  }

  const parents: string[] = [];
  let cursor: SyntaxNode | null = word ? word.prevSibling : node.name === "." ? node : null;
  while (cursor?.name === ".") {
    const qualifier = cursor.prevSibling;
    if (!isId(qualifier)) break;
    parents.unshift(idName(state, qualifier!));
    cursor = qualifier!.prevSibling;
  }

  // The whole reference (possibly `schema.table`), to look at what precedes it.
  let anchor: SyntaxNode | null = word ?? (node.name === "." ? node : null);
  while (anchor?.parent?.name === "CompositeIdentifier") anchor = anchor.parent;
  const statement = enclosingStatement(node, pos);
  const before = anchor ? previousToken(anchor) : lastTokenBefore(node.name === "Script" ? statement : node, pos);
  const clause = tableClause(state, before);

  return {
    ...base,
    from,
    quote,
    parents,
    empty: !word && parents.length === 0,
    tablePosition: clause !== null,
    clause,
    afterOn: parents.length === 0 && before?.name === "Keyword" && state.sliceDoc(before.from, before.to).toLowerCase() === "on",
    refs: statement ? statementRefs(state, statement) : [],
  };
}

/**
 * The statement the caret belongs to. With trailing whitespace the parser
 * ends the statement before the caret (`select * from |`), so a caret in
 * the script right after an unterminated statement still belongs to it.
 */
export function enclosingStatement(node: SyntaxNode, pos: number): SyntaxNode | null {
  for (let n: SyntaxNode | null = node; n; n = n.parent) if (n.name === "Statement") return n;
  if (node.name !== "Script") return null;
  const previous = lastTokenBefore(node, pos);
  if (previous?.name !== "Statement") return null;
  let last = previous.lastChild;
  while (last && isTrivia(last)) last = last.prevSibling;
  return last?.name === ";" ? null : previous;
}

function lastTokenBefore(container: SyntaxNode | null, pos: number): SyntaxNode | null {
  let child = container?.childBefore(pos) ?? null;
  while (child && isTrivia(child)) child = child.prevSibling;
  return child;
}

/**
 * Walks back over the table list (`from a x, b` …) to the clause keyword and
 * returns it. Operators, parentheses and anything else end the walk: not a
 * table spot.
 */
export function tableClause(state: EditorState, before: SyntaxNode | null): string | null {
  for (let token = before, steps = 0; token && steps < 64; token = previousToken(token), steps++) {
    if (token.name === "Keyword") {
      const word = state.sliceDoc(token.from, token.to).toLowerCase();
      if (TABLE_KEYWORDS.has(word)) return word;
      if (word === "as" || JOIN_MODIFIERS.has(word)) continue;
      return null;
    }
    const isListPart = isId(token) || token.name === "CompositeIdentifier" || state.sliceDoc(token.from, token.to) === ",";
    if (!isListPart) return null;
  }
  return null;
}

const JOIN_MODIFIERS = new Set(["left", "right", "inner", "outer", "full", "cross", "natural", "lateral", "only"]);
/** Keywords that end a statement's table list. */
const END_OF_REFS = new Set([
  "where", "group", "having", "order", "union", "intersect", "except", "limit", "offset", "fetch", "for",
  "set", "values", "returning", "window", "select",
]);

/** Tables named after FROM/JOIN/UPDATE/INTO in a statement, with their aliases. */
export function statementRefs(state: EditorState, statement: SyntaxNode): TableRef[] {
  const refs: TableRef[] = [];
  let mode: "none" | "refs" | "condition" = "none";
  let current: TableRef | null = null;
  for (let child = statement.firstChild; child; child = child.nextSibling) {
    if (isTrivia(child)) continue;
    if (child.name === "Keyword") {
      const word = state.sliceDoc(child.from, child.to).toLowerCase();
      if (TABLE_KEYWORDS.has(word)) {
        mode = "refs";
        current = null;
      } else if (word === "on" || word === "using") {
        mode = "condition";
      } else if (END_OF_REFS.has(word)) {
        mode = "none";
      }
      continue;
    }
    if (mode !== "refs") continue;
    if (isId(child) || child.name === "CompositeIdentifier") {
      if (!current) {
        current = { path: pathOf(state, child), from: child.from };
        refs.push(current);
      } else if (!current.alias && isId(child)) {
        current.alias = idName(state, child);
      }
    } else if (child.name === "Parens") {
      // A subquery: an alias may follow, but it has no known columns.
      current = { path: [], from: child.from };
    } else if (state.sliceDoc(child.from, child.to) === ",") {
      current = null;
    }
  }
  return refs.filter((r) => r.path.length > 0);
}

/* ------------------------------------------------------------------------ */
/* Icons                                                                    */
/* ------------------------------------------------------------------------ */

/** Lucide paths (ISC), matching the explorer's icons. */
const ICON_PATHS: Record<string, string> = {
  schema:
    '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  table: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18"/><path d="M9 3v18"/>',
  view: '<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/><circle cx="12" cy="12" r="3"/>',
  column: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="M15 3v18"/>',
  key: '<path d="M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"/><circle cx="16.5" cy="7.5" r=".5" fill="currentColor"/>',
  alias: '<circle cx="12" cy="12" r="4"/><path d="M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8"/>',
  join: '<path d="M9 17H7A5 5 0 0 1 7 7h2"/><path d="M15 7h2a5 5 0 1 1 0 10h-2"/><line x1="8" x2="16" y1="12" y2="12"/>',
};

/** Text glyphs for what lang-sql's keyword source produces. */
const GLYPHS: Record<string, string> = { keyword: "k", type: "t", variable: "f" };

/** Renders the leading icon of a completion option (see `autocompletion({ addToOptions })`). */
export function renderCompletionIcon(completion: Completion): Node {
  const types = (completion.type ?? "").split(" ");
  const kind = types.includes("key") ? "key" : types[0];
  const icon = document.createElement("span");
  icon.className = `idedb-completion-icon idedb-completion-icon-${kind || "none"}`;
  icon.setAttribute("aria-hidden", "true");
  const path = ICON_PATHS[kind];
  if (path) {
    icon.innerHTML = `<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${path}</svg>`;
  } else {
    icon.textContent = GLYPHS[kind] ?? "";
  }
  return icon;
}
