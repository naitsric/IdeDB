import { describe, expect, it } from "vitest";
import { codePointToUtf16, splitStatements, statementAt, statementsIn } from "./statements";

const texts = (sql: string, engine?: Parameters<typeof splitStatements>[1]) =>
  splitStatements(sql, engine).map((s) => s.text);

describe("splitStatements", () => {
  it("splits on semicolons and trims, skipping empty statements", () => {
    expect(texts("select 1;  select 2 ;\n\n;select 3")).toEqual(["select 1", "select 2", "select 3"]);
    expect(texts("")).toEqual([]);
    expect(texts(" ;; \n ; ")).toEqual([]);
  });

  it("reports absolute ranges without the semicolon", () => {
    const sql = "  select 1;\nselect 2";
    expect(splitStatements(sql)).toEqual([
      { text: "select 1", from: 2, to: 10 },
      { text: "select 2", from: 12, to: 20 },
    ]);
    for (const s of splitStatements(sql)) expect(sql.slice(s.from, s.to)).toBe(s.text);
  });

  it("ignores semicolons inside quotes and identifiers", () => {
    expect(texts(`select 'a;b', "c;d" from t; select 2`)).toEqual([`select 'a;b', "c;d" from t`, "select 2"]);
    expect(texts("select 'it''s; fine'; select 2")).toEqual(["select 'it''s; fine'", "select 2"]);
    expect(texts('select "a""; b"; select 2')).toEqual(['select "a""; b"', "select 2"]);
  });

  it("handles backslash escapes by dialect", () => {
    // MySQL: backslash escapes the quote.
    expect(texts("select 'it\\'s; ok'; select 2", "mysql")).toEqual(["select 'it\\'s; ok'", "select 2"]);
    // Postgres standard strings: backslash is literal, so the string ends at the second quote.
    expect(texts("select 'C:\\'; select 2", "postgres")).toEqual(["select 'C:\\'", "select 2"]);
    // Postgres E'' strings escape with backslash.
    expect(texts("select E'a\\'; b'; select 2", "postgres")).toEqual(["select E'a\\'; b'", "select 2"]);
    // SQLite never does.
    expect(texts("select 'C:\\'; select 2", "sqlite")).toEqual(["select 'C:\\'", "select 2"]);
  });

  it("treats backticks as identifiers only in MySQL", () => {
    expect(texts("select `a;b` from t; select 2", "mysql")).toEqual(["select `a;b` from t", "select 2"]);
    expect(texts("select `a;b`", "postgres")).toEqual(["select `a", "b`"]);
  });

  it("ignores semicolons in comments and leaves comments out of the range", () => {
    expect(texts("-- first; not a split\nselect 1; select 2 -- trailing; comment\n")).toEqual(["select 1", "select 2"]);
    expect(texts("/* a; b */ select 1 /* c; */; select 2")).toEqual(["select 1", "select 2"]);
    expect(texts("# note; here\nselect 1", "mysql")).toEqual(["select 1"]);
    expect(texts("-- only a comment;\n/* and; another */")).toEqual([]);
  });

  it("nests block comments only in Postgres", () => {
    expect(texts("/* a /* b; */ c; */ select 1", "postgres")).toEqual(["select 1"]);
    expect(texts("/* a /* b */ select 1; select 2", "sqlite")).toEqual(["select 1", "select 2"]);
  });

  it("skips Postgres dollar-quoted bodies", () => {
    const fn = "create function f() returns int as $$ begin; return 1; end $$ language plpgsql";
    expect(texts(`${fn}; select 2`)).toEqual([fn, "select 2"]);
    const tagged = "do $body$ begin; perform $$x;$$; end $body$";
    expect(texts(`${tagged}; select 2`)).toEqual([tagged, "select 2"]);
  });

  it("does not mistake parameters or identifiers for dollar quotes", () => {
    expect(texts("select $1; select $2")).toEqual(["select $1", "select $2"]);
    expect(texts("select a$b$ from t; select 2")).toEqual(["select a$b$ from t", "select 2"]);
  });

  it("runs unterminated quotes and comments to the end", () => {
    expect(texts("select 'open; select 2")).toEqual(["select 'open; select 2"]);
    expect(texts("select 1; /* open; select 2")).toEqual(["select 1"]);
    expect(texts("select $$ open; body")).toEqual(["select $$ open; body"]);
  });
});

describe("statementAt", () => {
  const sql = "select 1;\n\nselect 2;select 3";
  //           0123456789 0 1234567890123456789

  it("returns the statement containing the caret, ends inclusive", () => {
    expect(statementAt(sql, 0)?.text).toBe("select 1");
    expect(statementAt(sql, 8)?.text).toBe("select 1");
    expect(statementAt(sql, 11)?.text).toBe("select 2");
    expect(statementAt(sql, 20)?.text).toBe("select 3");
    expect(statementAt(sql, sql.length)?.text).toBe("select 3");
  });

  it("falls back to the nearest preceding statement between statements", () => {
    expect(statementAt(sql, 9)?.text).toBe("select 1");
    expect(statementAt(sql, 10)?.text).toBe("select 1");
    expect(statementAt("select 1; -- note\n", 15)?.text).toBe("select 1");
  });

  it("falls back to the first statement before any code", () => {
    expect(statementAt("\n\n  select 1; select 2", 0)?.text).toBe("select 1");
    expect(statementAt("-- comment\nselect 1", 3)?.text).toBe("select 1");
  });

  it("returns nothing for text without statements", () => {
    expect(statementAt("  -- nothing", 3)).toBeUndefined();
  });
});

describe("statementsIn", () => {
  it("splits a selection with ranges absolute to the document", () => {
    const sql = "select 0; select 1; select 2; select 3";
    const from = sql.indexOf("select 1");
    const to = sql.indexOf("select 3");
    const result = statementsIn(sql, from, to);
    expect(result.map((s) => s.text)).toEqual(["select 1", "select 2"]);
    for (const s of result) expect(sql.slice(s.from, s.to)).toBe(s.text);
  });
});

describe("codePointToUtf16", () => {
  it("counts astral characters as two units", () => {
    expect(codePointToUtf16("ab", 1)).toBe(1);
    expect(codePointToUtf16("😀x", 1)).toBe(2);
    expect(codePointToUtf16("ñandú x", 6)).toBe(6);
    expect(codePointToUtf16("ab", 10)).toBe(2);
  });
});
