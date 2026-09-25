import { EditorState } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";
import type { ColumnInfo, SchemaInfo, TableInfo } from "../db/api";
import type { SchemaLoad } from "../db/dataSources";
import { sqlLanguageSupport, type CompletionCatalog } from "./completion";
import { declarationAt } from "./navigation";

const column = (name: string): ColumnInfo => ({
  name,
  typeName: "int8",
  nullable: true,
  default: null,
  primaryKey: null,
  comment: null,
  generated: false,
});
const table = (name: string, columns: string[]): TableInfo => ({
  name,
  kind: "table",
  comment: null,
  columns: columns.map(column),
  foreignKeys: [],
});

const TABLES: Record<string, TableInfo[]> = {
  public: [table("audit_log", ["id", "note"])],
  shop: [table("orders", ["id", "customer_id"]), table("customers", ["id", "email"])],
};
const SCHEMAS: SchemaInfo[] = [
  { name: "public", isSystem: false },
  { name: "shop", isSystem: false },
];

function fakeCatalog(loaded = ["public", "shop"]) {
  let models: Record<string, SchemaLoad> = Object.fromEntries(
    loaded.map((s) => [s, { state: "loaded", model: { tables: TABLES[s] } } as SchemaLoad]),
  );
  const loadSchema = vi.fn(async (schema: string) => {
    models = { ...models, [schema]: { state: "loaded", model: { tables: TABLES[schema] ?? [] } } };
  });
  const catalog: CompletionCatalog = {
    snapshot: () => ({ engine: "postgres", defaultSchema: "public", schemas: SCHEMAS, showSystemSchemas: false, models }),
    loadSchema,
  };
  return { catalog, loadSchema };
}

/** `|` marks the caret. */
async function declarationOf(doc: string, catalog = fakeCatalog().catalog) {
  const pos = doc.indexOf("|");
  const state = EditorState.create({ doc: doc.replace("|", ""), extensions: [sqlLanguageSupport("postgres", catalog)] });
  return declarationAt(state, pos, catalog);
}

describe("go to declaration", () => {
  it("resolves a column behind an alias", async () => {
    expect(await declarationOf("select o.custo|mer_id from shop.orders o")).toEqual({
      schema: "shop",
      table: "orders",
      column: "customer_id",
    });
  });

  it("resolves the alias itself, where it is used and where it is declared", async () => {
    const orders = { schema: "shop", table: "orders" };
    expect(await declarationOf("select |o.id from shop.orders o")).toEqual(orders);
    expect(await declarationOf("select o.id from shop.orders |o")).toEqual(orders);
  });

  it("resolves a table and its schema in the FROM list", async () => {
    expect(await declarationOf("select * from shop.ord|ers")).toEqual({ schema: "shop", table: "orders" });
    expect(await declarationOf("select * from sh|op.orders")).toEqual({ schema: "shop" });
    expect(await declarationOf("select * from audit|_log")).toEqual({ schema: "public", table: "audit_log" });
  });

  it("resolves a bare column against the statement's tables", async () => {
    expect(await declarationOf("select em|ail from shop.customers c join shop.orders o on o.customer_id = c.id")).toEqual({
      schema: "shop",
      table: "customers",
      column: "email",
    });
  });

  it("resolves a column qualified by its table name", async () => {
    expect(await declarationOf("select orders.i|d from shop.orders")).toEqual({ schema: "shop", table: "orders", column: "id" });
  });

  it("loads a schema that is not introspected yet", async () => {
    const { catalog, loadSchema } = fakeCatalog(["public"]);
    expect(await declarationOf("select * from shop.custo|mers", catalog)).toEqual({ schema: "shop", table: "customers" });
    expect(loadSchema).toHaveBeenCalledWith("shop");
  });

  it("finds nothing for keywords, literals and unknown names", async () => {
    expect(await declarationOf("sel|ect 1")).toBeNull();
    expect(await declarationOf("select 'or|ders'")).toBeNull();
    expect(await declarationOf("select * from nowh|ere")).toBeNull();
  });
});
