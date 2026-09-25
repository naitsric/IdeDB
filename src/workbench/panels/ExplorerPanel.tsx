import { Plus, RefreshCw, SquareTerminal } from "lucide-react";
import { executeCommand } from "../../commands/registry";
import { useDataSources } from "../../db/dataSources";
import { DatabaseTree } from "../../explorer/DatabaseTree";
import { Button, IconButton } from "../../ui/primitives";

export function ExplorerPanel() {
  const { sources, loaded } = useDataSources();

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-border px-1.5">
        <IconButton label="New Data Source" shortcut="$mod+KeyN" onClick={() => executeCommand("datasource.new")}>
          <Plus className="size-4" />
        </IconButton>
        <IconButton label="Refresh" shortcut="$mod+Alt+KeyY" onClick={() => executeCommand("explorer.refresh")}>
          <RefreshCw className="size-3.5" />
        </IconButton>
        <IconButton label="New Query Console" shortcut="$mod+Shift+KeyL" onClick={() => executeCommand("console.new")}>
          <SquareTerminal className="size-3.5" />
        </IconButton>
      </div>
      {loaded && sources.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <p className="text-[12px] text-muted">No data sources yet.</p>
          <Button variant="primary" onClick={() => executeCommand("datasource.new")}>
            <Plus className="size-3.5" /> New Data Source
          </Button>
        </div>
      ) : (
        <DatabaseTree />
      )}
    </div>
  );
}
