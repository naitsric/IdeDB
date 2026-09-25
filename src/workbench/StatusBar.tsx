import { isRunning, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { ENGINE_LABEL } from "../ui/EngineIcon";
import { StatusDot } from "../ui/primitives";
import { useWorkbench } from "./bridge";

export function StatusBar() {
  const connected = useDataSources((s) => Object.values(s.explorers).filter((e) => e.status === "connected").length);
  const activeConsoleId = useWorkbench((s) => s.activeConsoleId);
  const dataSourceId = useConsoles((s) => (activeConsoleId ? s.consoles[activeConsoleId]?.dataSourceId : undefined));
  const running = useConsoles((s) => isRunning(activeConsoleId ? s.consoles[activeConsoleId] : undefined));
  const server = useDataSources((s) => (dataSourceId ? s.explorers[dataSourceId]?.server : undefined));

  return (
    <footer className="flex h-[var(--statusbar-height)] shrink-0 items-center gap-3 border-t border-border bg-bg px-3 text-[11px] text-subtle">
      <span className="flex items-center gap-1.5">
        <StatusDot tone={connected > 0 ? "success" : "idle"} />
        {connected === 0 ? "No connections" : `${connected} connected`}
      </span>
      {running && <span className="text-accent">Executing…</span>}
      {server && (
        <span className="ml-auto">
          {ENGINE_LABEL[server.engine]} {server.version}
        </span>
      )}
    </footer>
  );
}
