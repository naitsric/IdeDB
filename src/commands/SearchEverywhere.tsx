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
import { useT, type MessageKey } from "../i18n";
import { categoryLabel, commandTitle, executeCommand, isEnabled, useCommands, type Command } from "./registry";

export type SearchScope = "all" | "actions" | "tables";

const PLACEHOLDER: Record<SearchScope, MessageKey> = {
  all: "search.placeholder.all",
  actions: "search.placeholder.actions",
  tables: "search.placeholder.tables",
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
  const t = useT();
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
      label={t("titlebar.searchEverywhere")}
      loop
      overlayClassName={paletteOverlayClass}
      contentClassName={paletteContentClass}
    >
      <div className="flex items-center gap-2 border-b border-border px-3">
        <Search className="size-4 text-subtle" />
        <Cmdk.Input
          autoFocus
          placeholder={t(PLACEHOLDER[scope])}
          className={paletteInputClass}
        />
        <Kbd>esc</Kbd>
      </div>
      <Cmdk.List className={paletteListClass}>
        <Cmdk.Empty className="px-3 py-6 text-center text-muted">
          {scope === "tables" && tables.length === 0 ? t("search.connectFirst") : t("search.nothingFound")}
        </Cmdk.Empty>

        {scope !== "actions" && tables.length > 0 && (
          <Cmdk.Group heading={t("search.tables")} className={paletteGroupClass}>
            {tables.map((tb) => (
              <Cmdk.Item
                key={tb.key}
                value={`table ${tb.name} ${tb.schema} ${tb.key}`}
                onSelect={() => run(() => openTableData(tb.sourceId, tb.schema, tb.name))}
                className={paletteItemClass}
              >
                <span className="flex min-w-0 items-center gap-2">
                  {tb.view ? <Eye className="size-3.5 shrink-0" /> : <Table2 className="size-3.5 shrink-0" />}
                  <span className="truncate">{tb.name}</span>
                </span>
                <span className="muted truncate text-[12px] text-subtle">
                  {tb.schema} · {tb.sourceName}
                </span>
              </Cmdk.Item>
            ))}
          </Cmdk.Group>
        )}

        {scope !== "tables" &&
          [...groups].map(([category, items]) => (
            <Cmdk.Group key={category} heading={categoryLabel(category)} className={paletteGroupClass}>
              {items.map((command) => (
                <Cmdk.Item
                  key={command.id}
                  value={`${categoryLabel(command.category)} ${commandTitle(command)} ${command.id}`}
                  keywords={command.keywords}
                  disabled={!isEnabled(command)}
                  onSelect={() => run(() => executeCommand(command.id))}
                  className={paletteItemClass}
                >
                  <span className="truncate">{commandTitle(command)}</span>
                  {command.keybinding && <Kbd binding={command.keybinding} />}
                </Cmdk.Item>
              ))}
            </Cmdk.Group>
          ))}
      </Cmdk.List>
    </Cmdk.Dialog>
  );
}
