import { describe, expect, it } from "vitest";
import { sqlLiteral, tableQuery } from "../db/sql";
import { aggregate, formatAggregates } from "./aggregate";
import { toCsv, toInserts, toJson, toTsv } from "./format";

const rows = [
  [1, "plain", null],
  [2n ** 62n, 'has "quotes", commas\nand lines', new Uint8Array([0xde, 0xad])],
];

describe("clipboard and export formats", () => {
  it("writes TSV and CSV, quoting only fields that need it", () => {
    expect(toTsv(rows)).toBe('1\tplain\t\n4611686018427387904\t"has ""quotes"", commas\nand lines"\t0xDEAD');
    expect(toCsv(rows, ["id", "text", "bin"])).toBe(
      'id,text,bin\n1,plain,\n4611686018427387904,"has ""quotes"", commas\nand lines",0xDEAD',
    );
  });

  it("writes JSON with exact big integers and hex bytes", () => {
    expect(JSON.parse(toJson(rows, ["id", "text", "bin"]))).toEqual([
      { id: 1, text: "plain", bin: null },
      { id: "4611686018427387904", text: 'has "quotes", commas\nand lines', bin: "0xDEAD" },
    ]);
    expect(toJson([], ["id"])).toBe("[]");
  });

  it("writes one INSERT per row with engine quoting", () => {
    const table = { schema: "shop", name: "order" };
    expect(toInserts("postgres", table, "public", ["id", "select"], [[1, "it's"]])).toBe(
      `insert into shop."order" (id, "select") values (1, 'it''s');`,
    );
    expect(toInserts("mysql", table, "shop", ["id"], [[null]])).toBe("insert into `order` (id) values (NULL);");
    expect(toInserts("sqlite", null, null, ["b"], [[new Uint8Array([1])]])).toBe(
      "insert into my_table (b) values (X'01');",
    );
  });
});

describe("SQL text", () => {
  it("renders literals per engine", () => {
    expect(sqlLiteral("postgres", "a\\b'c")).toBe("'a\\b''c'");
    expect(sqlLiteral("mysql", "a\\b'c")).toBe("'a\\\\b''c'");
    expect(sqlLiteral("postgres", new Uint8Array([0xab]))).toBe("'\\xab'::bytea");
    expect(sqlLiteral("sqlite", true)).toBe("TRUE");
    expect(sqlLiteral("mysql", 12345678901234567890n)).toBe("12345678901234567890");
  });

  it("builds the data editor query from the filter bar", () => {
    expect(tableQuery("postgres", "shop", "orders", "public", { where: " status = 'paid' ", orderBy: "id desc" })).toBe(
      "select * from shop.orders where status = 'paid' order by id desc",
    );
    expect(tableQuery("mysql", "shop", "orders", "shop", { where: "  " })).toBe("select * from orders");
  });
});

describe("selection aggregates", () => {
  it("summarizes numeric selections, including numeric text", () => {
    const result = aggregate([1, "2.5", null, 10n]);
    expect(result).toEqual({ count: 4, numeric: { sum: 13.5, avg: 4.5, min: 1, max: 10 } });
    expect(formatAggregates(result)).toBe("4 cells · sum 13.5 · avg 4.5 · min 1 · max 10");
  });

  it("only counts when any value is not a number", () => {
    expect(aggregate([1, "abc"])).toEqual({ count: 2 });
    expect(formatAggregates({ count: 1 })).toBe("1 cell");
  });
});
