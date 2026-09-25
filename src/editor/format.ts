import { formatDialect, mysql, postgresql, sqlite, type DialectOptions } from "sql-formatter";
import type { Engine } from "../db/api";

const DIALECT: Record<Engine, DialectOptions> = { postgres: postgresql, mysql, sqlite };

/**
 * Reformats SQL in the engine's dialect, keeping the user's keyword case.
 * Returns `null` when the text cannot be parsed (the formatter bails out on
 * syntax it does not know), so the caller leaves it untouched.
 */
export function formatSql(text: string, engine: Engine): string | null {
  try {
    return formatDialect(text, { dialect: DIALECT[engine], keywordCase: "preserve", tabWidth: 2 });
  } catch {
    return null;
  }
}
