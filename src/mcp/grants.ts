import type { DataSource } from "../db/api";
import type { Access, Grant } from "./api";

/** A client's access to one data source, as the grants matrix shows it. */
export type GrantLevel = "none" | Access;

export const GRANT_LEVELS: readonly GrantLevel[] = ["none", "read", "write"];

/**
 * Why IdeDB can't serve a data source over MCP, if it can't. Mirrors
 * `unavailable` in crates/idedb-mcp/src/tools.rs, which tells the model
 * the same in `list_connections`.
 */
export function unavailableReason(source: DataSource): string | null {
  const { engine, path } = source.params;
  if (engine === "sqlite") {
    return path.trim().startsWith("file:")
      ? "Its path is a file: URI, which IdeDB can't open read-only. Use a plain file path."
      : null;
  }
  return source.savePassword ? null : "Its password isn't saved, and MCP clients never get asked for one.";
}

export function grantLevel(grants: readonly Grant[], dataSourceId: string): GrantLevel {
  return grants.find((g) => g.dataSourceId === dataSourceId)?.access ?? "none";
}

/** The grants with one data source set to `level`, the others untouched. */
export function withGrant(grants: readonly Grant[], dataSourceId: string, level: GrantLevel): Grant[] {
  const others = grants.filter((g) => g.dataSourceId !== dataSourceId);
  return level === "none" ? others : [...others, { dataSourceId, access: level }];
}

/**
 * Why `level` can't be chosen for a data source, or null when it can.
 * Lowering access is always possible, even where the data source can't be
 * served, so a grant left from before can be removed.
 */
export function levelBlocked(
  level: GrantLevel,
  current: GrantLevel,
  context: { revoked: boolean; unavailable: string | null; neverWrite: boolean },
): string | null {
  if (level === current) return null;
  if (context.revoked) return "A revoked client gets no access.";
  if (level === "none") return null;
  if (context.unavailable) return context.unavailable;
  if (level === "write" && context.neverWrite && current !== "write") {
    return "Never write is on for this data source.";
  }
  return null;
}
