import { Command as Cmdk } from "cmdk";
import { History } from "lucide-react";
import { useEffect, useState } from "react";
import { create } from "zustand";
import { api, errorMessage, type HistoryEntry } from "../db/api";
import { useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { editorFor } from "../editor/registry";
import {
  paletteContentClass,
  paletteInputClass,
  paletteItemClass,
  paletteListClass,
  paletteOverlayClass,
} from "../ui/palette";
import { formatDuration } from "../ui/format";
import { Kbd } from "../ui/primitives";

export const useHistoryPalette = create<{ consoleId?: string; show: (consoleId: string) => void; hide: () => void }>(
  (set) => ({
    show: (consoleId) => set({ consoleId }),
    hide: () => set({ consoleId: undefined }),
  }),
);

/** Enough to cover a working session; the store keeps more. */
const LIMIT = 500;

/**
 * Query History (⌥⌘E): statements run against the active console's data
 * source, newest first. Picking one inserts it at the caret.
 */
export function HistoryPalette() {
  const { consoleId, hide } = useHistoryPalette();
  const dataSourceId = useConsoles((s) => (consoleId ? s.consoles[consoleId]?.dataSourceId : undefined));
  const sourceName = useDataSources((s) => s.sources.find((x) => x.id === dataSourceId)?.name);
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!dataSourceId) return;
    setEntries(null);
    setError(undefined);
    api
      .history(dataSourceId, null, LIMIT)
      .then((all) => setEntries(dedupe(all)))
      .catch((e) => setError(errorMessage(e)));
  }, [consoleId, dataSourceId]);

  const insert = (sql: string) => {
    const target = consoleId;
    hide();
    // Let the dialog unmount and return focus before editing.
    requestAnimationFrame(() => target && editorFor(target)?.insertAtCaret(sql));
  };

  return (
    <Cmdk.Dialog
      open={consoleId !== undefined}
      onOpenChange={(open) => !open && hide()}
      label="Query History"
      loop
      // Substring match: fuzzy scoring ranks SQL poorly.
      filter={(value, search) => (value.toLowerCase().includes(search.toLowerCase()) ? 1 : 0)}
      overlayClassName={paletteOverlayClass}
      contentClassName={paletteContentClass}
    >
      <div className="flex items-center gap-2 border-b border-border px-3">
        <History className="size-4 text-subtle" />
        <Cmdk.Input autoFocus placeholder={`Search history of ${sourceName ?? "this data source"}…`} className={paletteInputClass} />
        <Kbd>esc</Kbd>
      </div>
      <Cmdk.List className={paletteListClass}>
        {error ? (
          <div className="selectable px-3 py-6 text-center text-danger">{error}</div>
        ) : entries === null ? (
          <Cmdk.Loading className="px-3 py-6 text-center text-muted">Loading…</Cmdk.Loading>
        ) : (
          <Cmdk.Empty className="px-3 py-6 text-center text-muted">
            {entries.length === 0 ? "Nothing run against this data source yet." : "Nothing found"}
          </Cmdk.Empty>
        )}
        {entries?.map((entry) => (
          <Cmdk.Item
            key={entry.id}
            value={`${entry.sql} #${entry.id}`}
            onSelect={() => insert(entry.sql)}
            className={paletteItemClass}
          >
            <span className="flex min-w-0 items-center gap-2">
              <span
                className={`size-1.5 shrink-0 rounded-full ${entry.error ? "bg-danger" : "bg-success"}`}
                title={entry.error ?? undefined}
              />
              <span className="truncate font-mono text-[12px]">{entry.sql.replace(/\s+/g, " ")}</span>
            </span>
            <span className="muted shrink-0 text-[11.5px] text-subtle tabular-nums">
              {relativeTime(entry.executedAt)}
              {entry.elapsedMs !== null && ` · ${formatDuration(entry.elapsedMs)}`}
            </span>
          </Cmdk.Item>
        ))}
      </Cmdk.List>
    </Cmdk.Dialog>
  );
}

/** Keeps the newest run of each distinct statement. */
function dedupe(entries: HistoryEntry[]): HistoryEntry[] {
  const seen = new Set<string>();
  return entries.filter((e) => {
    const key = e.sql.trim();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

const relative = new Intl.RelativeTimeFormat("en", { numeric: "auto", style: "short" });

function relativeTime(iso: string): string {
  const seconds = Math.round((Date.parse(iso) - Date.now()) / 1000);
  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ];
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) return relative.format(Math.round(seconds / size), unit);
  }
  return "just now";
}
