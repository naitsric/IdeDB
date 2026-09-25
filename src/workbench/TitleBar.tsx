import { Moon, Search, Sun, SunMoon } from "lucide-react";
import { useSearchEverywhere } from "../commands/SearchEverywhere";
import { useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { useTheme, type ThemePreference } from "../theme";
import { EngineIcon } from "../ui/EngineIcon";
import { IconButton, Kbd } from "../ui/primitives";
import { useWorkbench } from "./bridge";

const NEXT_THEME: Record<ThemePreference, ThemePreference> = { system: "light", light: "dark", dark: "system" };
const THEME_ICON = { system: SunMoon, light: Sun, dark: Moon };

/**
 * Custom title bar drawn under the overlay-style native one: the macOS
 * traffic lights sit on the left and the whole bar drags the window.
 */
export function TitleBar() {
  const show = useSearchEverywhere((s) => s.show);
  const { preference, setPreference } = useTheme();
  const activeConsoleId = useWorkbench((s) => s.activeConsoleId);
  const sourceId = useConsoles((s) => (activeConsoleId ? s.consoles[activeConsoleId]?.dataSourceId : undefined));
  const source = useDataSources((s) => s.sources.find((x) => x.id === sourceId));
  const ThemeIcon = THEME_ICON[preference];

  return (
    <header
      data-tauri-drag-region
      className="flex h-[var(--titlebar-height)] shrink-0 items-center gap-3 border-b border-border bg-titlebar pr-2 pl-[var(--traffic-lights-inset)]"
    >
      <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center gap-2 text-[12.5px]">
        <span data-tauri-drag-region className="font-semibold text-fg">
          IdeDB
        </span>
        {source && (
          <span data-tauri-drag-region className="flex min-w-0 items-center gap-1.5 text-muted">
            <span className="text-subtle">/</span>
            <EngineIcon engine={source.params.engine} />
            <span className="truncate">{source.name}</span>
          </span>
        )}
      </div>

      <button
        type="button"
        onClick={() => show("all")}
        className="flex h-6 w-[min(360px,40vw)] items-center gap-2 rounded-md border border-border bg-inset px-2 text-[12px] text-subtle transition-colors hover:border-border-strong hover:text-muted"
      >
        <Search className="size-3.5" />
        Search Everywhere
        <span className="ml-auto">
          <Kbd>⇧⇧</Kbd>
        </span>
      </button>

      <div data-tauri-drag-region className="flex flex-1 justify-end">
        <IconButton label={`Theme: ${preference}`} onClick={() => setPreference(NEXT_THEME[preference])}>
          <ThemeIcon className="size-4" />
        </IconButton>
      </div>
    </header>
  );
}
