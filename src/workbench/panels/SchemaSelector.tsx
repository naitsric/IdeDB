import { FolderTree } from "lucide-react";
import { effectiveSchema, useConsoles } from "../../db/consoles";
import { useDataSources } from "../../db/dataSources";

/**
 * The console's current schema (Postgres search path head, MySQL current
 * database), as in DataGrip's console toolbar. Hidden for SQLite, which
 * resolves names across attached databases, and until schemas are known.
 */
export function SchemaSelector({ consoleId }: { consoleId: string }) {
  const entry = useConsoles((s) => s.consoles[consoleId]);
  const setSchema = useConsoles((s) => s.setSchema);
  const source = useDataSources((s) => s.sources.find((x) => x.id === entry?.dataSourceId));
  const explorer = useDataSources((s) => (entry ? s.explorers[entry.dataSourceId] : undefined));
  const showSystemSchemas = useDataSources((s) => s.showSystemSchemas);

  if (!entry || source?.params.engine === "sqlite" || explorer?.status !== "connected" || !explorer.schemas) return null;

  const serverDefault = explorer.server?.defaultSchema ?? null;
  const current = effectiveSchema(entry);
  const options = explorer.schemas
    .filter((s) => showSystemSchemas || !s.isSystem || s.name === current)
    .map((s) => s.name);

  return (
    <label className="ml-2 flex shrink-0 items-center gap-1 text-[12px] text-muted" title="Current schema">
      <FolderTree className="size-3.5 text-subtle" />
      <select
        aria-label="Current schema"
        value={current ?? ""}
        onChange={(e) => void setSchema(consoleId, e.target.value === serverDefault ? undefined : e.target.value)}
        className="h-6 max-w-40 rounded-md border border-transparent bg-transparent px-1 text-[12px] text-fg outline-none hover:border-border focus:border-accent"
      >
        {current === null && <option value="">(none)</option>}
        {options.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
    </label>
  );
}
