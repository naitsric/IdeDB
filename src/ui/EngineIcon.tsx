import { Database } from "lucide-react";
import type { Engine } from "../db/api";
import { cx } from "./primitives";

export const ENGINE_LABEL: Record<Engine, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  sqlite: "SQLite",
};

const ENGINE_COLOR: Record<Engine, string> = {
  postgres: "text-[#5b8def]",
  mysql: "text-[#e48e00]",
  sqlite: "text-[#3fa7d6]",
};

/** A generic database glyph tinted per engine; no vendor logos. */
export function EngineIcon({ engine, className }: { engine: Engine; className?: string }) {
  return <Database className={cx("size-3.5 shrink-0", ENGINE_COLOR[engine], className)} />;
}
