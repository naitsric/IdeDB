import { describe, expect, it } from "vitest";
import { qualifiedName, quoteIdent, selectAll } from "./sql";

describe("identifier quoting", () => {
  it("quotes each engine's own keywords", () => {
    for (const name of ["window", "array", "only", "fetch", "user", "table"]) {
      expect(quoteIdent("postgres", name)).toBe(`"${name}"`);
    }
    for (const name of ["rank", "rows", "groups", "window", "key", "order"]) {
      expect(quoteIdent("mysql", name)).toBe(`\`${name}\``);
    }
    for (const name of ["rows", "groups", "window", "order", "transaction"]) {
      expect(quoteIdent("sqlite", name)).toBe(`"${name}"`);
    }
  });

  it("leaves words an engine does not reserve bare", () => {
    expect(quoteIdent("postgres", "rank")).toBe("rank");
    expect(quoteIdent("sqlite", "rank")).toBe("rank");
    expect(quoteIdent("mysql", "only")).toBe("only");
    expect(quoteIdent("postgres", "orders")).toBe("orders");
  });

  it("quotes names that are not plain identifiers, escaping the quote character", () => {
    expect(quoteIdent("postgres", "Orders")).toBe('"Orders"');
    expect(quoteIdent("mysql", "Orders")).toBe("Orders");
    expect(quoteIdent("postgres", 'a"b')).toBe('"a""b"');
    expect(quoteIdent("mysql", "a`b")).toBe("`a``b`");
    expect(quoteIdent("sqlite", "order items")).toBe('"order items"');
  });

  it("builds table queries that open reserved-word tables", () => {
    expect(selectAll("mysql", "shop", "rank", "shop")).toBe("select * from `rank`");
    expect(qualifiedName("postgres", "analytics", "window", "public")).toBe('analytics."window"');
  });
});
