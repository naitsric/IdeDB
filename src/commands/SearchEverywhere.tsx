import { Command as Cmdk } from "cmdk";
import { Eye, Search, Table2 } from "lucide-react";
import { useMemo } from "react";
import { create } from "zustand";
import { openTableData } from "../actions";
import { useDataSources } from "../db/dataSources";
import {
  paletteContentClass,
  paletteGroupClass,
  paletteInputClass,
  paletteItemClass,
  paletteListClass,
  paletteOverlayClass,
} from "../ui/palette";
import { Kbd } from "../ui/primitives";
import { executeCommand, isEnabled, useCommands, type Command } from "./registry";

export type SearchScope = "all" | "actions" | "tables";

const PLACEHOLDER: Record<SearchScope, string> = {
  all: "Search tables and actions…",
  actions: "Search actions…",
  tables: "Go to table…",
};

export const useSearchEverywhere = create<{ open: boolean; scope: SearchScope; show: (scope: SearchScope) => void; hide: () => void }>(
  (set) => ({
    open: false,
    scope: "all",
    show: (scope) => set({ open: true, scope }),
    hide: () => set({ open: false }),
  }),
);

/** Upper bound on table rows rendered; the filter narrows long before that matters. */
const MAX_TABLES = 5000;

/**
 * Search Everywhere (⇧⇧), Find Action (⌘⇧A) and Go to Table (⌘O): one
 * palette with scopes. Tables come from every schema the explorer has
 * introspected.
 */
export function SearchEverywhere() {
  const { open, scope, hide } = useSearchEverywhere();
  const commands = useCommands((s) => s.commands);
  const sources = useDataSources((s) => s.sources);
  const explorers = useDataSources((s) => s.explorers);

  const tables = useMemo(() => {
    const items: { key: string; sourceId: string; sourceName: string; schema: string; name: string; view: boolean }[] = [];
    for (const source of sources) {
      for (const [schema, load] of Object.entries(explorers[source.id]?.models ?? {})) {
        if (load.state !== "loaded") continue;
        for (const table of load.model.tables) {
          items.push({
            key: `${source.id}/${schema}/${table.name}`,
            sourceId: source.id,
            sourceName: source.name,
            schema,
            name: table.name,
            view: table.kind === "view" || table.kind === "materializedView",
          });
        }
      }
    }
    return items.slice(0, MAX_TABLES);
  }, [sources, explorers]);

  const groups = new Map<string, Command[]>();
  for (const command of Object.values(commands)) {
    groups.set(command.category, [...(groups.get(command.category) ?? []), command]);
  }

  // Let the dialog unmount and focus return before the action runs.
  const run = (action: () => void) => {
    hide();
    requestAnimationFrame(action);
  };

  return (
    <Cmdk.Dialog
      open={open}
      onOpenChange={(o) => !o && hide()}
      label="Search Everywhere"
      loop
      overlayClassName={paletteOverlayClass}
      contentClassName={paletteContentClass}
    >
      <div className="flex items-center gap-2 border-b border-border px-3">
        <Search className="size-4 text-subtle" />
        <Cmdk.Input
          autoFocus
          placeholder={PLACEHOLDER[scope]}
          className={paletteInputClass}
        />
        <Kbd>esc</Kbd>
      </div>
      <Cmdk.List className={paletteListClass}>
        <Cmdk.Empty className="px-3 py-6 text-center text-muted">
          {scope === "tables" && tables.length === 0 ? "Connect a data source to search its tables." : "Nothing found"}
        </Cmdk.Empty>

        {scope !== "actions" && tables.length > 0 && (
          <Cmdk.Group heading="Tables" className={paletteGroupClass}>
            {tables.map((t) => (
              <Cmdk.Item
                key={t.key}
                value={`table ${t.name} ${t.schema} ${t.key}`}
                onSelect={() => run(() => openTableData(t.sourceId, t.schema, t.name))}
                className={paletteItemClass}
              >
                <span className="flex min-w-0 items-center gap-2">
                  {t.view ? <Eye className="size-3.5 shrink-0" /> : <Table2 className="size-3.5 shrink-0" />}
                  <span className="truncate">{t.name}</span>
                </span>
                <span className="muted truncate text-[12px] text-subtle">
                  {t.schema} · {t.sourceName}
                </span>
              </Cmdk.Item>
            ))}
          </Cmdk.Group>
        )}

        {scope !== "tables" &&
          [...groups].map(([category, items]) => (
            <Cmdk.Group key={category} heading={category} className={paletteGroupClass}>
              {items.map((command) => (
                <Cmdk.Item
                  key={command.id}
                  value={`${command.category} ${command.title} ${command.id}`}
                  keywords={command.keywords}
                  disabled={!isEnabled(command)}
                  onSelect={() => run(() => executeCommand(command.id))}
                  className={paletteItemClass}
                >
                  <span className="truncate">{command.title}</span>
                  {command.keybinding && <Kbd binding={command.keybinding} />}
                </Cmdk.Item>
              ))}
            </Cmdk.Group>
          ))}
      </Cmdk.List>
    </Cmdk.Dialog>
  );
}
