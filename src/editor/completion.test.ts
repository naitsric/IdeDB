import { CompletionContext, type Completion, type CompletionResult, type CompletionSource } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";
import type { ColumnInfo, Engine, SchemaInfo, TableInfo } from "../db/api";
import type { SchemaLoad } from "../db/dataSources";
import { aliasFor, analyze, sqlLanguageSupport, type CompletionCatalog } from "./completion";

const column = (name: string, typeName = "int4", primaryKey: number | null = null): ColumnInfo => ({
  name,
  typeName,
  nullable: true,
  default: null,
  primaryKey,
  comment: null,
});

const table = (name: string, columns: ColumnInfo[], kind: TableInfo["kind"] = "table"): TableInfo => ({
  name,
  kind,
  comment: null,
  columns,
  foreignKeys: [],
});

const TABLES: Record<string, TableInfo[]> = {
  public: [table("audit_log", [column("id", "int8", 1)])],
  shop: [
    table("customers", [column("id", "int8", 1), column("email", "text")]),
    table("orders", [column("id", "int8", 1), column("customer_id", "int8"), column("total", "numeric")]),
    table("Order Items", [column("qty")]),
    table("order_totals", [column("total")], "view"),
  ],
};

const SCHEMAS: SchemaInfo[] = [
  { name: "public", isSystem: false },
  { name: "shop", isSystem: false },
  { name: "pg_catalog", isSystem: true },
];

/** A catalog whose `loaded` schemas are introspected up front; the rest load on demand. */
function fakeCatalog({ loaded = ["public"], connected = true, loadDelay = 0 } = {}) {
  let models: Record<string, SchemaLoad> = Object.fromEntries(
    loaded.map((s) => [s, { state: "loaded", model: { tables: TABLES[s] ?? [] } } as SchemaLoad]),
  );
  const loadSchema = vi.fn(async (schema: string) => {
    await new Promise((r) => setTimeout(r, loadDelay));
    models = { ...models, [schema]: { state: "loaded", model: { tables: TABLES[schema] ?? [] } } };
  });
  const catalog: CompletionCatalog = {
    snapshot: () =>
      connected
        ? { engine: "postgres", defaultSchema: "public", schemas: SCHEMAS, showSystemSchemas: false, models }
        : null,
    loadSchema,
  };
  return { catalog, loadSchema };
}

/** `|` marks the caret. */
function setup(doc: string, catalog: CompletionCatalog, engine: Engine = "postgres") {
  const pos = doc.indexOf("|");
  const state = EditorState.create({
    doc: doc.replace("|", ""),
    selection: { anchor: pos },
    extensions: [sqlLanguageSupport(engine, catalog)],
  });
  return { state, pos };
}

/**
 * Runs every completion source the language registers and returns the
 * options matching what was typed, best first (boost, then label), which is
 * roughly how CodeMirror ranks equally good matches.
 */
async function complete(doc: string, catalog: CompletionCatalog, { explicit = false } = {}) {
  const { state, pos } = setup(doc, catalog);
  const sources = state.languageDataAt<CompletionSource>("autocomplete", pos);
  const results = (await Promise.all(sources.map((s) => s(new CompletionContext(state, pos, explicit))))).filter(
    (r): r is CompletionResult => !!r,
  );
  const options: Completion[] = [];
  for (const result of results) {
    const typed = state.sliceDoc(result.from, pos).toLowerCase();
    options.push(...result.options.filter((o) => o.label.toLowerCase().startsWith(typed)));
  }
  return options.sort((a, b) => (b.boost ?? 0) - (a.boost ?? 0) || a.label.localeCompare(b.label));
}

const labels = (options: Completion[]) => options.map((o) => o.label);

describe("schema completion", () => {
  it("offers schemas in table position before they are introspected, without keywords", async () => {
    const { catalog, loadSchema } = fakeCatalog();
    const options = await complete("select * from sh|", catalog);
    expect(labels(options)).toEqual(["shop"]);
    expect(options[0].type).toBe("schema");
    expect(loadSchema).not.toHaveBeenCalled();
  });

  it("loads a schema on demand after `schema.` and offers its tables", async () => {
    const { catalog, loadSchema } = fakeCatalog({ loadDelay: 5 });
    const options = await complete("select * from shop.|", catalog);
    expect(loadSchema).toHaveBeenCalledExactlyOnceWith("shop");
    expect(labels(options)).toEqual(["customers", "Order Items", "order_totals", "orders"]);
    expect(options.find((o) => o.label === "order_totals")?.type).toBe("view");
    expect(options.find((o) => o.label === "Order Items")?.apply).toBe('"Order Items"');
  });

  it("narrows tables of a loaded schema while typing", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    expect(labels(await complete("select * from shop.cu|", catalog))).toEqual(["customers"]);
  });

  it("offers columns after an alias, loading the aliased table's schema", async () => {
    const { catalog, loadSchema } = fakeCatalog();
    const options = await complete("select o.| from shop.orders o", catalog);
    expect(loadSchema).toHaveBeenCalledWith("shop");
    expect(labels(options)).toEqual(["customer_id", "id", "total"]);
    expect(options.find((o) => o.label === "id")?.type).toBe("column key");
    expect(options.find((o) => o.label === "total")?.detail).toBe("numeric");
  });

  it("offers columns after a table name used in the statement", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    const options = await complete("select orders.cu| from shop.orders", catalog);
    expect(labels(options)).toEqual(["customer_id"]);
  });

  it("qualifies tables outside the default schema and ranks default-schema tables first", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    const options = await complete("select * from |", catalog, { explicit: true });
    expect(options[0].label).toBe("audit_log");
    expect(options[0].apply).toBeUndefined();
    const customers = options.find((o) => o.label === "customers")!;
    expect(customers.apply).toBe("shop.customers");
    expect(customers.detail).toBe("shop");
    expect(labels(options)).not.toContain("pg_catalog");
  });

  it("treats every table slot of the FROM list and joins as table position", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    expect(labels(await complete("select * from shop.orders o, cus|", catalog))).toEqual(["customers"]);
    expect(labels(await complete("select * from shop.orders o left join cus|", catalog))).toEqual(["customers"]);
  });

  it("keeps keywords outside table positions and ranks the statement's columns above them", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    const options = await complete("select * from shop.orders where to|", catalog);
    expect(options[0]).toMatchObject({ label: "total", type: "column" });
    expect(options.some((o) => o.type === "keyword")).toBe(true);
  });

  it("falls back to keywords only while the data source is not connected", async () => {
    const { catalog } = fakeCatalog({ connected: false });
    const options = await complete("select * from sh|", catalog);
    expect(options.length).toBeGreaterThan(0);
    expect(options.every((o) => o.type === "keyword" || o.type === "type" || o.type === "variable")).toBe(true);
  });

  it("returns nothing when the request was aborted while a schema loaded", async () => {
    const { catalog } = fakeCatalog({ loadDelay: 10 });
    const { state, pos } = setup("select * from shop.|", catalog);
    const [source] = state.languageDataAt<CompletionSource>("autocomplete", pos);
    const context = new CompletionContext(state, pos, false);
    const pending = source(context);
    // What CodeMirror does when the user keeps typing: drop the abort listeners.
    (context as unknown as { abortListeners: null }).abortListeners = null;
    expect(await pending).toBeNull();
  });

  it("does not complete inside strings or comments", async () => {
    const { catalog } = fakeCatalog({ loaded: ["public", "shop"] });
    expect(await complete("select 'sh|'", catalog, { explicit: true })).toEqual([]);
    expect(await complete("select 1 -- from sh|", catalog, { explicit: true })).toEqual([]);
  });
});

describe("analyze", () => {
  const at = (doc: string) => {
    const { state, pos } = setup(doc, fakeCatalog().catalog);
    return analyze(state, pos);
  };

  it("detects table positions", () => {
    expect(at("select * from |").tablePosition).toBe(true);
    expect(at("update sh|").tablePosition).toBe(true);
    expect(at("insert into sh|").tablePosition).toBe(true);
    expect(at("select * from a as x join sh|").tablePosition).toBe(true);
    expect(at("select sh|").tablePosition).toBe(false);
    expect(at("select * from a where sh|").tablePosition).toBe(false);
    expect(at("select * from a join b on b.id = sh|").tablePosition).toBe(false);
  });

  it("collects qualifiers and statement tables with aliases", () => {
    const doc = "select o.| from shop.orders o join customers as c on c.id = o.customer_id";
    const result = at(doc);
    const text = doc.replace("|", "");
    expect(result.parents).toEqual(["o"]);
    expect(result.refs).toEqual([
      { path: ["shop", "orders"], alias: "o", from: text.indexOf("shop.orders") },
      { path: ["customers"], alias: "c", from: text.indexOf("customers") },
    ]);
  });

  it("knows the clause keyword and a spot right after ON", () => {
    expect(at("select * from a join |").clause).toBe("join");
    expect(at("select * from |").clause).toBe("from");
    expect(at("select * from a join b on |").afterOn).toBe(true);
    expect(at("select * from a join b on b.|").afterOn).toBe(false);
  });
});

describe("join completion", () => {
  const fk = (columns: string[], referencedTable: string, referencedColumns: string[], referencedSchema = "shop") => ({
    name: `fk_${columns.join("_")}`,
    columns,
    referencedSchema,
    referencedTable,
    referencedColumns,
  });
  const withKeys = (t: TableInfo, foreignKeys: TableInfo["foreignKeys"]) => ({ ...t, foreignKeys });

  /** shop: orders → customers; order_items → orders (composite key) and → products. */
  function joinCatalog() {
    const models: Record<string, SchemaLoad> = {
      public: { state: "loaded", model: { tables: [] } },
      shop: {
        state: "loaded",
        model: {
          tables: [
            table("customers", [column("id", "int8", 1), column("email", "text")]),
            withKeys(table("orders", [column("tenant", "int4", 1), column("id", "int8", 2), column("customer_id", "int8")]), [
              fk(["customer_id"], "customers", ["id"]),
            ]),
            withKeys(
              table("order_items", [column("tenant"), column("order_id", "int8"), column("sku", "text")]),
              [fk(["tenant", "order_id"], "orders", ["tenant", "id"]), fk(["sku"], "products", ["sku"])],
            ),
            table("products", [column("sku", "text", 1)]),
          ],
        },
      },
    };
    const catalog: CompletionCatalog = {
      snapshot: () => ({ engine: "postgres", defaultSchema: "shop", schemas: SCHEMAS, showSystemSchemas: false, models }),
      loadSchema: async () => {},
    };
    return catalog;
  }

  it("suggests tables linked by foreign keys, both directions, as full join snippets", async () => {
    const options = await complete("select * from orders o join |", joinCatalog(), { explicit: false });
    const joins = options.filter((o) => o.type === "join");
    expect(joins.map((o) => o.apply)).toEqual([
      "customers c on c.id = o.customer_id",
      "order_items oi on oi.tenant = o.tenant and oi.order_id = o.id",
    ]);
    // Joins rank above plain tables.
    expect(options[0].type).toBe("join");
  });

  it("narrows join snippets while typing the table name, and keeps aliases unique", async () => {
    const options = await complete("select * from order_items oi join or|", joinCatalog());
    expect(options.filter((o) => o.type === "join").map((o) => o.apply)).toEqual([
      "orders o on o.tenant = oi.tenant and o.id = oi.order_id",
    ]);
    const taken = await complete("select * from orders c join cu|", joinCatalog());
    expect(taken.find((o) => o.type === "join")?.apply).toBe("customers c1 on c1.id = c.customer_id");
  });

  it("qualifies joined tables outside the default schema", async () => {
    const catalog = joinCatalog();
    const snapshot = catalog.snapshot()!;
    const other: CompletionCatalog = { ...catalog, snapshot: () => ({ ...snapshot, defaultSchema: "public" }) };
    const options = await complete("select * from shop.orders o join cu|", other);
    expect(options.find((o) => o.type === "join")?.apply).toBe("shop.customers c on c.id = o.customer_id");
  });

  it("suggests the foreign key condition right after ON", async () => {
    const options = await complete("select * from orders o join order_items i on |", joinCatalog());
    expect(options.map((o) => o.label)).toEqual(["i.tenant = o.tenant and i.order_id = o.id"]);
    const reversed = await complete("select * from order_items i join orders o on |", joinCatalog());
    expect(reversed.map((o) => o.label)).toEqual(["o.tenant = i.tenant and o.id = i.order_id"]);
  });

  it("makes aliases from word initials, skipping reserved words and taken names", () => {
    expect(aliasFor("order_items", new Set())).toBe("oi");
    expect(aliasFor("orderItems", new Set())).toBe("oi");
    expect(aliasFor("order_rules", new Set())).toBe("or1");
    expect(aliasFor("customers", new Set(["c", "c1"]))).toBe("c2");
  });
});
