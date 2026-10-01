import { describe, expect, it } from "vitest";
import type { DataSource, Engine } from "../db/api";
import type { Grant } from "./api";
import { grantLevel, levelBlocked, unavailableReason, withGrant } from "./grants";

const source = (engine: Engine, extra: { savePassword?: boolean; path?: string } = {}): DataSource => ({
  id: "ds",
  name: "db",
  params: { engine, host: "localhost", port: null, user: "u", database: "", sslMode: "prefer", path: extra.path ?? "" },
  color: null,
  savePassword: extra.savePassword ?? true,
});

describe("unavailableReason", () => {
  it("serves server data sources only with a saved password", () => {
    expect(unavailableReason(source("postgres"))).toBeNull();
    expect(unavailableReason(source("mysql", { savePassword: false }))).toMatch(/password isn't saved/);
    expect(unavailableReason(source("postgres", { savePassword: false }))).not.toBeNull();
  });

  it("serves SQLite files but not file: URIs", () => {
    expect(unavailableReason(source("sqlite", { path: "/tmp/app.db", savePassword: false }))).toBeNull();
    expect(unavailableReason(source("sqlite", { path: "  file:/tmp/app.db?mode=ro" }))).toMatch(/file: URI/);
  });
});

describe("grants", () => {
  const grants: Grant[] = [
    { dataSourceId: "a", access: "read" },
    { dataSourceId: "b", access: "write" },
  ];

  it("reads a data source's level", () => {
    expect(grantLevel(grants, "a")).toBe("read");
    expect(grantLevel(grants, "b")).toBe("write");
    expect(grantLevel(grants, "c")).toBe("none");
  });

  it("sets one data source and leaves the rest", () => {
    expect(withGrant(grants, "a", "write")).toEqual([
      { dataSourceId: "b", access: "write" },
      { dataSourceId: "a", access: "write" },
    ]);
    expect(withGrant(grants, "b", "none")).toEqual([{ dataSourceId: "a", access: "read" }]);
    expect(withGrant(grants, "c", "read")).toHaveLength(3);
    expect(withGrant([], "c", "none")).toEqual([]);
  });
});

describe("levelBlocked", () => {
  const open = { revoked: false, unavailable: null, neverWrite: false };

  it("allows every level on a data source that can be served", () => {
    for (const level of ["none", "read", "write"] as const) expect(levelBlocked(level, "none", open)).toBeNull();
  });

  it("only lowers access where the data source can't be served", () => {
    const unavailable = { ...open, unavailable: "no password" };
    expect(levelBlocked("read", "none", unavailable)).toBe("no password");
    expect(levelBlocked("write", "read", unavailable)).toBe("no password");
    expect(levelBlocked("none", "write", unavailable)).toBeNull();
    expect(levelBlocked("write", "write", unavailable)).toBeNull();
  });

  it("grants no new write on a never-write data source", () => {
    const locked = { ...open, neverWrite: true };
    expect(levelBlocked("write", "read", locked)).toMatch(/Never write/);
    expect(levelBlocked("read", "write", locked)).toBeNull();
    expect(levelBlocked("read", "none", locked)).toBeNull();
  });

  it("changes nothing for a revoked client", () => {
    const revoked = { ...open, revoked: true };
    expect(levelBlocked("none", "read", revoked)).toMatch(/revoked/);
    expect(levelBlocked("read", "none", revoked)).toMatch(/revoked/);
    expect(levelBlocked("read", "read", revoked)).toBeNull();
  });
});
