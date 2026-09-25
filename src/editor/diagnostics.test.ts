// @vitest-environment happy-dom
import { forEachDiagnostic, type Diagnostic } from "@codemirror/lint";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SqlProblem } from "../db/api";
import {
  createdBefore,
  createdInDocument,
  createdName,
  hasUnpreparableParameters,
  missingTable,
  sqlDiagnostics,
  type LintBackend,
} from "./diagnostics";
import { editorApi } from "./editorApi";
import { statementsField } from "./extensions";
import { splitStatements } from "./statements";

describe("parameters the engine cannot prepare", () => {
  it("flags named parameters outside strings and comments", () => {
    expect(hasUnpreparableParameters("select * from t where id = :id", "postgres")).toBe(true);
    expect(hasUnpreparableParameters("select * from t where id = :id", "mysql")).toBe(true);
    expect(hasUnpreparableParameters("select ':id', \"a:b\" -- :id\n from t", "postgres")).toBe(false);
    expect(hasUnpreparableParameters("select $$ :id $$", "postgres")).toBe(false);
  });

  it("ignores casts, slices and what the engine prepares itself", () => {
    expect(hasUnpreparableParameters("select x::text, a[1:2] from t", "postgres")).toBe(false);
    expect(hasUnpreparableParameters("select * from t where id = $1", "postgres")).toBe(false);
    expect(hasUnpreparableParameters("select * from t where id = ?", "mysql")).toBe(false);
    expect(hasUnpreparableParameters("select * from t where id = :id", "sqlite")).toBe(false);
  });
});

describe("tables created in the document", () => {
  it("reads the name a CREATE TABLE or VIEW makes", () => {
    expect(createdName("create table t (id int)")).toBe("t");
    expect(createdName("CREATE TEMP TABLE IF NOT EXISTS shop.Orders (id int)")).toBe("orders");
    expect(createdName('create or replace view "My View" as select 1')).toBe("my view");
    expect(createdName("create materialized view mv as select 1")).toBe("mv");
    expect(createdName("create unlogged table `x`.y (id int)")).toBe("y");
    expect(createdName("create index i on t (id)")).toBeNull();
    expect(createdName("select 1")).toBeNull();
  });

  it("only counts CREATEs before the statement", () => {
    const statements = splitStatements("select * from a; create table a (id int); create view b as select 1");
    expect([...createdBefore(statements, 0)]).toEqual([]);
    expect([...createdBefore(statements, 2)]).toEqual(["a"]);
    expect([...createdBefore(statements, 3)]).toEqual(["a", "b"]);
  });

  it("reads the missing table from each engine's message", () => {
    expect(missingTable('ERROR: relation "shop.t" does not exist')).toBe("t");
    expect(missingTable("ERROR 1146 (42S02): Table 'idedb.t' doesn't exist")).toBe("t");
    expect(missingTable("no such table: main.t")).toBe("t");
    expect(missingTable('ERROR: column "x" does not exist')).toBeNull();
  });

  it("suppresses a missing-table problem only when an earlier statement creates it", () => {
    const statements = splitStatements("create table t (id int); select * from t; select * from u");
    const missing = (name: string): SqlProblem => ({ message: `ERROR: relation "${name}" does not exist`, position: 14 });
    expect(createdInDocument(missing("t"), statements, 1)).toBe(true);
    expect(createdInDocument(missing("u"), statements, 2)).toBe(false);
    expect(createdInDocument({ message: "ERROR: syntax error", position: 0 }, statements, 1)).toBe(false);
  });
});

describe("live diagnostics", () => {
  let view: EditorView | undefined;
  afterEach(() => {
    view?.destroy();
    vi.useRealTimers();
  });

  function setup(doc: string, backend: LintBackend) {
    const statements = statementsField("postgres");
    view = new EditorView({
      parent: document.body,
      state: EditorState.create({ doc, extensions: [statements, sqlDiagnostics(statements, "postgres", backend)] }),
    });
    return { view, api: editorApi(view, "postgres", statements) };
  }

  function diagnostics(v: EditorView) {
    const found: Diagnostic[] = [];
    forEachDiagnostic(v.state, (d, from, to) => found.push({ ...d, from, to }));
    return found;
  }

  /** Lets the lint delay pass and the checks resolve. */
  async function settle() {
    await vi.advanceTimersByTimeAsync(1000);
  }

  let sessions = 0;
  /**
   * Flags `nope` like a server reporting an unknown column, and `from t`
   * like a missing table. Each backend gets its own context, so cached
   * results from another test never leak in.
   */
  function fakeBackend(context: string | null = `session-${++sessions}`) {
    const check = vi.fn(async (sql: string): Promise<SqlProblem | null> => {
      if (sql.includes("from t") && !sql.startsWith("create")) return { message: 'ERROR: relation "t" does not exist', position: sql.indexOf("t") };
      const at = sql.indexOf("nope");
      return at < 0 ? null : { message: 'ERROR: column "nope" does not exist', position: at };
    });
    return { backend: { context: () => context, check } as LintBackend, check };
  }

  it("underlines what the engine reports, after typing stops", async () => {
    vi.useFakeTimers();
    const { backend, check } = fakeBackend();
    const doc = "select 1;\nselect nope from ok";
    const { view: v } = setup(doc, backend);
    expect(diagnostics(v)).toEqual([]);
    await settle();
    const [d] = diagnostics(v);
    expect(v.state.sliceDoc(d.from, d.to)).toBe("nope");
    expect(d.message).toContain("nope");
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("does not recheck unchanged statements after an edit", async () => {
    vi.useFakeTimers();
    const { backend, check } = fakeBackend();
    const { view: v } = setup("select 1;\nselect 2", backend);
    await settle();
    v.dispatch({ changes: { from: v.state.doc.length, insert: "3" } });
    await settle();
    expect(check.mock.calls.map(([sql]) => sql)).toEqual(["select 1", "select 2", "select 23"]);
  });

  it("does not flag a table created earlier in the console, nor check while disconnected", async () => {
    vi.useFakeTimers();
    const { backend } = fakeBackend();
    const { view: v } = setup("create table t (id int);\nselect * from t", backend);
    await settle();
    expect(diagnostics(v)).toEqual([]);

    view!.destroy();
    const offline = fakeBackend(null);
    const { view: w } = setup("select nope", offline.backend);
    await settle();
    expect(diagnostics(w)).toEqual([]);
    expect(offline.check).not.toHaveBeenCalled();
  });

  it("keeps the execution error next to live problems, and clears both on edit", async () => {
    vi.useFakeTimers();
    const { backend } = fakeBackend();
    const { view: v, api } = setup("select nope;\nselect 1 / 0", backend);
    await settle();
    const [, statement] = splitStatements(v.state.doc.toString());
    api.showError(statement, "ERROR: division by zero", 11);
    await settle();
    expect(diagnostics(v).map((d) => v.state.sliceDoc(d.from, d.to)).sort()).toEqual(["0", "nope"]);
    v.dispatch({ changes: { from: 0, insert: " " } });
    await Promise.resolve();
    expect(diagnostics(v)).toEqual([]);
    // Live problems come back once typing stops; the execution error does not.
    await settle();
    expect(diagnostics(v).map((d) => v.state.sliceDoc(d.from, d.to))).toEqual(["nope"]);
  });
});
