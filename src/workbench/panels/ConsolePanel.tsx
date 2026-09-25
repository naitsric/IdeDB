import type { IDockviewPanelProps } from "dockview-react";
import { Play, Square } from "lucide-react";
import { useEffect, useMemo } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { executeCommand } from "../../commands/registry";
import { activeResult, isRunning, rowGetter, useConsoles } from "../../db/consoles";
import { useDataSources } from "../../db/dataSources";
import { catalogFor } from "../../editor/catalog";
import { lintBackendFor, useLintContext } from "../../editor/lintBackend";
import { editorFor } from "../../editor/registry";
import { SqlEditor } from "../../editor/SqlEditor";
import { ResultView } from "../../grid/ResultView";
import { EngineIcon } from "../../ui/EngineIcon";
import { IconButton, Kbd, StatusDot } from "../../ui/primitives";
import { ResultTabs } from "./ResultTabs";
import { SchemaSelector } from "./SchemaSelector";
import { TransactionControls } from "./TransactionControls";

const noRows = () => undefined;

/** A query console bound to one data source: SQL editor on top, result tabs below. */
export function ConsolePanel({ params, api }: IDockviewPanelProps<{ consoleId: string }>) {
  const { consoleId } = params;
  const entry = useConsoles((s) => s.consoles[consoleId]);
  const setSql = useConsoles((s) => s.setSql);
  const source = useDataSources((s) => s.sources.find((x) => x.id === entry?.dataSourceId));
  const dataSourceId = entry?.dataSourceId;
  const engine = source?.params.engine;
  const catalog = useMemo(
    () => (dataSourceId && engine ? catalogFor(consoleId, dataSourceId, engine) : undefined),
    [consoleId, dataSourceId, engine],
  );
  const lint = useMemo(() => lintBackendFor(consoleId), [consoleId]);

  // Recheck the statements on screen when what checks run against changes.
  const lintContext = useLintContext(consoleId);
  useEffect(() => editorFor(consoleId)?.refreshDiagnostics(), [consoleId, lintContext]);

  const title = entry?.table?.name ?? source?.name ?? "console";
  useEffect(() => api.setTitle(title), [api, title]);

  // Focus the editor whenever this tab becomes active.
  useEffect(() => {
    const focus = () => requestAnimationFrame(() => editorFor(consoleId)?.focus());
    if (api.isActive) focus();
    const sub = api.onDidActiveChange(({ isActive }) => isActive && focus());
    return () => sub.dispose();
  }, [api, consoleId]);

  if (!entry || !source) {
    return (
      <div className="flex h-full items-center justify-center bg-panel text-[12px] text-subtle">
        This console's data source no longer exists.
      </div>
    );
  }

  const running = isRunning(entry);
  const result = activeResult(entry);
  const sessionTone = entry.connecting ? "warning" : entry.sessionId !== undefined ? "success" : "idle";

  return (
    <div className="flex h-full flex-col bg-panel">
      {source.color && <div className="h-0.5 shrink-0" style={{ background: source.color }} />}
      <div className="flex h-9 shrink-0 items-center gap-1 border-b border-border px-2">
        <IconButton
          label="Execute"
          shortcut="$mod+Enter"
          disabled={running || entry.connecting}
          onClick={() => executeCommand("console.execute")}
          className="text-success hover:text-success"
        >
          <Play className="size-4 fill-current" />
        </IconButton>
        <IconButton
          label="Cancel"
          shortcut="$mod+F2"
          disabled={!running}
          onClick={() => executeCommand("console.cancel")}
          className="text-danger hover:text-danger"
        >
          <Square className="size-3.5 fill-current" />
        </IconButton>
        <div className="ml-2 flex min-w-0 items-center gap-1.5 text-[12px] text-muted">
          <EngineIcon engine={source.params.engine} />
          <span className="truncate">{source.name}</span>
          <StatusDot tone={sessionTone} />
        </div>
        <SchemaSelector consoleId={consoleId} />
        <TransactionControls consoleId={consoleId} />
        {entry.connectError && (
          <span className="selectable ml-2 truncate text-[12px] text-danger" title={entry.connectError}>
            {entry.connectError}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2 text-[11px] text-subtle">
          Run <Kbd binding="$mod+Enter" />
          History <Kbd binding="$mod+Alt+KeyE" />
        </div>
      </div>
      <Group orientation="vertical" className="min-h-0 flex-1">
        <Panel id="editor" defaultSize="40%" minSize="10%">
          <SqlEditor
            consoleId={consoleId}
            engine={source.params.engine}
            initialValue={entry.sql}
            onChange={(sql) => setSql(consoleId, sql)}
            catalog={catalog!}
            lint={lint}
          />
        </Panel>
        <Separator className="h-px bg-border transition-colors data-[separator=active]:bg-accent data-[separator=hover]:bg-accent" />
        <Panel id="results" minSize="15%">
          <div className="flex h-full flex-col">
            {entry.results.length > 0 && (
              <ResultTabs consoleId={consoleId} results={entry.results} activeResultId={entry.activeResultId} />
            )}
            <div className="min-h-0 flex-1">
              <ResultView consoleId={consoleId} result={result} getRow={result ? rowGetter(result.id) : noRows} />
            </div>
          </div>
        </Panel>
      </Group>
    </div>
  );
}
