import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { TableInfo } from "../db/api";
import {
  enclosingStatement,
  findSchema,
  idName,
  isId,
  previousToken,
  resolveTable,
  statementRefs,
  tableClause,
  type CatalogSnapshot,
  type CompletionCatalog,
  type SyntaxNode,
  type TableRef,
} from "./completion";

/** What a name in the editor refers to, as a path in the Database Explorer. */
export interface Declaration {
  schema: string;
  table?: string;
  column?: string;
}

/**
 * Go to Declaration (⌘B): resolves the name under the caret to the schema,
 * table or column it refers to. Aliases resolve to their table; a bare
 * column resolves against the statement's tables; schemas that are not
 * introspected yet are loaded first.
 */
export async function declarationAt(
  state: EditorState,
  pos: number,
  catalog: CompletionCatalog,
): Promise<Declaration | null> {
  const node = identifierAt(state, pos);
  if (!node || !catalog.snapshot()) return null;

  const { parts, index, anchor } = pathAround(state, node);
  const statement = enclosingStatement(anchor, pos);
  const refs = statement ? statementRefs(state, statement) : [];
  const inTableList = tableClause(state, previousToken(anchor)) !== null;
  return resolve(catalog, refs, parts, index, inTableList);
}

function identifierAt(state: EditorState, pos: number): SyntaxNode | null {
  const tree = syntaxTree(state);
  for (const side of [1, -1] as const) {
    const node = tree.resolveInner(pos, side);
    if (isId(node)) return node;
  }
  return null;
}

/** The dotted name a node belongs to (`shop.orders.id`), and which part the node is. */
function pathAround(state: EditorState, node: SyntaxNode) {
  if (node.parent?.name === "CompositeIdentifier") {
    const ids: SyntaxNode[] = [];
    for (let child = node.parent.firstChild; child; child = child.nextSibling) if (isId(child)) ids.push(child);
    return { parts: ids.map((n) => idName(state, n)), index: ids.findIndex((n) => n.from === node.from), anchor: node.parent };
  }
  // Incomplete statements may leave `a.b` as siblings joined by "." instead.
  const before: SyntaxNode[] = [];
  for (let cur = node; cur.prevSibling?.name === "." && isId(cur.prevSibling.prevSibling); ) {
    cur = cur.prevSibling.prevSibling!;
    before.unshift(cur);
  }
  const after: SyntaxNode[] = [];
  for (let cur = node; cur.nextSibling?.name === "." && isId(cur.nextSibling.nextSibling); ) {
    cur = cur.nextSibling.nextSibling!;
    after.push(cur);
  }
  const nodes = [...before, node, ...after];
  return { parts: nodes.map((n) => idName(state, n)), index: before.length, anchor: nodes[0] };
}

const same = (a: string | undefined, b: string) => !!a && a.toLowerCase() === b.toLowerCase();

async function resolve(
  catalog: CompletionCatalog,
  refs: readonly TableRef[],
  parts: string[],
  index: number,
  inTableList: boolean,
): Promise<Declaration | null> {
  const snapshot = () => catalog.snapshot() as CatalogSnapshot;

  /** A table by path, loading its schema first when needed. */
  const tableAt = async (path: string[]) => {
    let resolved = resolveTable(snapshot(), path);
    if (resolved === "unloaded") {
      const schema = findSchema(snapshot(), path[0]);
      if (schema) await catalog.loadSchema(schema);
      resolved = catalog.snapshot() ? resolveTable(snapshot(), path) : null;
    }
    return resolved === "unloaded" ? null : resolved;
  };
  /** An alias of the statement, or a table it names, or any table by that path. */
  const tableOrAlias = async (qualifier: string[]) => {
    if (qualifier.length === 1) {
      const ref =
        refs.find((r) => same(r.alias, qualifier[0])) ?? refs.find((r) => !r.alias && same(r.path.at(-1), qualifier[0]));
      if (ref) return tableAt(ref.path);
    }
    return tableAt(qualifier);
  };
  const aliasOf = async (name: string) => {
    const ref = refs.find((r) => same(r.alias, name));
    return ref ? tableAt(ref.path) : null;
  };
  const asTable = (t: { schema: string; table: TableInfo } | null): Declaration | null =>
    t && { schema: t.schema, table: t.table.name };
  const asSchema = (name: string): Declaration | null => {
    const schema = findSchema(snapshot(), name);
    return schema ? { schema } : null;
  };
  const withColumn = (t: { schema: string; table: TableInfo }, name: string): Declaration | null => {
    const column = t.table.columns.find((c) => c.name === name) ?? t.table.columns.find((c) => same(c.name, name));
    return column ? { schema: t.schema, table: t.table.name, column: column.name } : null;
  };

  if (parts.length === 1) {
    const [name] = parts;
    if (inTableList) return asTable(await tableAt([name])) ?? asTable(await aliasOf(name)) ?? asSchema(name);
    const aliased = await aliasOf(name);
    if (aliased) return asTable(aliased);
    for (const ref of refs) {
      const table = await tableAt(ref.path);
      const column = table && withColumn(table, name);
      if (column) return column;
    }
    return asTable(await tableAt([name])) ?? asSchema(name);
  }

  if (index < parts.length - 1) {
    // A qualifier: `shop` in `shop.orders`, `o` in `o.id`.
    const qualifier = parts.slice(0, index + 1);
    if (qualifier.length > 1) return asTable(await tableAt(qualifier));
    if (inTableList) return asSchema(qualifier[0]) ?? asTable(await tableOrAlias(qualifier));
    return asTable(await tableOrAlias(qualifier)) ?? asSchema(qualifier[0]);
  }

  // The last part: a table after its schema, or a column after its table.
  if (inTableList) return asTable(await tableAt(parts));
  const table = await tableOrAlias(parts.slice(0, -1));
  if (table) return withColumn(table, parts.at(-1)!) ?? asTable(table);
  return asTable(await tableAt(parts.slice(-2)));
}
