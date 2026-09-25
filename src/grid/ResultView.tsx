import type { GridSelection, Item } from "@glideapps/glide-data-grid";
import { CircleAlert, CircleCheck, CircleSlash, LoaderCircle } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, type ReactNode } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import type { Value } from "../db/api";
import { useConsoles, type ResultMeta } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { ContextMenuRoot, ContextMenuTrigger } from "../ui/ContextMenu";
import { formatDuration } from "../ui/format";
import { editCell, visibleRowCount } from "./actions";
import { aggregate, formatAggregates } from "./aggregate";
import { editorTarget, EMPTY_SELECTION, selectionCells, useGrids, type EditorTarget } from "./dataEditor";
import { cellValue, changeCount, DEFAULT, NO_EDITS } from "./edits";
import { GridMenu } from "./GridMenu";
import { ResultGrid } from "./ResultGrid";
import { TableBar } from "./TableBar";
import { ValueViewer } from "./ValueViewer";

const count = new Intl.NumberFormat("en-US");
const MAX_AGGREGATED_CELLS = 1_000_000;

/**
 * A statement's outcome: status line plus the grid, the affected-rows notice
 * or the error. A table's data gets the data editor on top: filter bar and
 * editing. Memoized: the console re-renders on every keystroke.
 */
export const ResultView = memo(function ResultView({
  consoleId,
  result,
  getRow,
}: {
  consoleId: string;
  result?: ResultMeta;
  getRow: (index: number) => Value[] | undefined;
}) {
  const entry = useConsoles((s) => s.consoles[consoleId]);
  const schema = result?.table?.schema;
  const load = useDataSources((s) => (entry && schema ? s.explorers[entry.dataSourceId]?.models[schema] : undefined));
  const explorerConnected = useDataSources((s) => !!entry && s.explorers[entry.dataSourceId]?.status === "connected");
  const submitting = useGrids((s) => (result ? !!s.submitting[result.id] : false));
  // `load` makes the target follow the introspected structure as it arrives;
  // `submitting` makes the grid read-only while a submit runs.
  const target = useMemo(() => editorTarget(entry, result), [entry, result, load, submitting]);

  // Editing and foreign key navigation need the table's structure.
  useEffect(() => {
    if (entry && schema && explorerConnected && !load) void useDataSources.getState().loadSchema(entry.dataSourceId, schema);
  }, [entry, schema, explorerConnected, load]);

  return (
    <div className="flex h-full flex-col bg-panel">
      {result?.table && <TableBar consoleId={consoleId} result={result} target={target} />}
      <ResultStatus result={result} getRow={getRow} />
      <div className="relative min-h-0 flex-1">
        {!result ? (
          <Empty>Run a statement to see results here.</Empty>
        ) : result.status === "error" && result.columns.length === 0 ? (
          <pre className="selectable m-3 rounded-md bg-danger/10 p-3 font-mono text-[12px] whitespace-pre-wrap text-danger">
            {result.error}
          </pre>
        ) : result.columns.length === 0 && result.status !== "running" ? (
          <Empty>
            {result.affectedRows !== undefined
              ? `${count.format(result.affectedRows)} ${result.affectedRows === 1 ? "row" : "rows"} affected`
              : "Statement executed"}
          </Empty>
        ) : (
          <ResultBody consoleId={consoleId} result={result} getRow={getRow} target={target} />
        )}
      </div>
    </div>
  );
});

function ResultBody({
  consoleId,
  result,
  getRow,
  target,
}: {
  consoleId: string;
  result: ResultMeta;
  getRow: (index: number) => Value[] | undefined;
  target: EditorTarget | undefined;
}) {
  const resultId = result.id;
  const editable = target?.editable === true;
  const edits = useGrids((s) => s.edits[resultId]);
  const selection = useGrids((s) => s.selections[resultId]) ?? EMPTY_SELECTION;
  const viewer = useGrids((s) => s.valueViewer);

  const activate = useCallback(() => {
    const active = useGrids.getState().active;
    if (active?.resultId !== resultId) useGrids.setState({ active: { consoleId, resultId } });
  }, [consoleId, resultId]);

  const onSelectionChange = useCallback(
    (next: GridSelection) => useGrids.setState((s) => ({ selections: { ...s.selections, [resultId]: next } })),
    [resultId],
  );

  // Right-clicking outside the selection selects the clicked cell first, like a native list.
  const onCellContextMenu = useCallback(
    ([col, row]: Item) => {
      activate();
      const current = useGrids.getState().selections[resultId];
      const range = current?.current?.range;
      const inside =
        current?.rows.hasIndex(row) ||
        (range && col >= range.x && col < range.x + range.width && row >= range.y && row < range.y + range.height);
      if (!inside && col >= 0 && row >= 0) {
        onSelectionChange({
          ...EMPTY_SELECTION,
          current: { cell: [col, row], range: { x: col, y: row, width: 1, height: 1 }, rangeStack: [] },
        });
      }
    },
    [activate, onSelectionChange, resultId],
  );

  const onCellEdit = useCallback(
    (row: number, col: number, value: Value) => editCell(consoleId, resultId, row, col, value),
    [consoleId, resultId],
  );

  return (
    <Group orientation="horizontal" className="size-full">
      <Panel id="grid" minSize="30%">
        <ContextMenuRoot>
          <ContextMenuTrigger asChild>
            <div data-focus-context="grid" className="size-full" onFocusCapture={activate} onPointerDownCapture={activate}>
              <ResultGrid
                resultId={resultId}
                columns={result.columns}
                rowCount={result.rowCount}
                getRow={getRow}
                version={result.version}
                edits={result.table ? (edits ?? NO_EDITS) : undefined}
                editable={editable}
                onCellEdit={onCellEdit}
                selection={selection}
                onSelectionChange={onSelectionChange}
                onCellContextMenu={onCellContextMenu}
              />
            </div>
          </ContextMenuTrigger>
          <GridMenu isTable={!!result.table} editable={editable} />
        </ContextMenuRoot>
      </Panel>
      {viewer && (
        <>
          <Separator className="w-px bg-border transition-colors data-[separator=active]:bg-accent data-[separator=hover]:bg-accent" />
          <Panel id="viewer" defaultSize="32%" minSize="15%">
            <ValueViewer consoleId={consoleId} result={result} target={target} />
          </Panel>
        </>
      )}
    </Group>
  );
}

function ResultStatus({ result, getRow }: { result?: ResultMeta; getRow: (index: number) => Value[] | undefined }) {
  const resultId = result?.id ?? -1;
  const selection = useGrids((s) => s.selections[resultId]);
  const edits = useGrids((s) => s.edits[resultId]) ?? NO_EDITS;
  const failure = useGrids((s) => s.failures[resultId]);
  const notice = useGrids((s) => s.notices[resultId]);

  // Aggregates of the selected cells, pending edits included.
  const selected = useMemo(() => {
    if (!result || !selection) return undefined;
    const cells = selectionCells(selection, visibleRowCount(result), result.columns.length);
    const size = cells ? cells.rows.length * cells.cols.length : 0;
    if (!cells || size < 2) return undefined;
    // Beyond this, summing on every selection change would make the grid stutter.
    if (size > MAX_AGGREGATED_CELLS) return formatAggregates({ count: size });
    function* values() {
      for (const row of cells!.rows) {
        for (const col of cells!.cols) {
          const { value } = cellValue(edits, result!.rowCount, getRow, row, col);
          yield value === DEFAULT ? null : value;
        }
      }
    }
    return formatAggregates(aggregate(values()));
  }, [result, selection, edits, getRow]);

  if (!result) return <div className="h-8 shrink-0 border-b border-border" />;

  const icon = {
    running: <LoaderCircle className="size-3.5 animate-spin text-accent" />,
    done: <CircleCheck className="size-3.5 text-success" />,
    cancelled: <CircleSlash className="size-3.5 text-warning" />,
    error: <CircleAlert className="size-3.5 text-danger" />,
  }[result.status];

  const pending = changeCount(edits);
  const parts = [
    result.columns.length > 0 && `${count.format(result.rowCount)} ${result.rowCount === 1 ? "row" : "rows"}`,
    result.firstPageMs !== undefined && `first page ${result.firstPageMs} ms`,
    result.elapsedMs !== undefined && formatDuration(result.elapsedMs),
    result.status === "cancelled" && "cancelled",
  ].filter(Boolean);

  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-3 text-[12px] text-muted">
      {icon}
      <span className="shrink-0 tabular-nums">{parts.join(" · ")}</span>
      {pending > 0 && (
        <span className="shrink-0 rounded bg-accent-soft px-1.5 py-px text-[11px] text-accent">
          {pending} pending {pending === 1 ? "change" : "changes"}
        </span>
      )}
      {selected && <span className="shrink-0 text-subtle tabular-nums">{selected}</span>}
      {result.status === "error" && result.columns.length > 0 && (
        <span className="selectable truncate text-danger">{result.error}</span>
      )}
      {failure && (
        <span className="selectable truncate text-danger" title={failure.message}>
          {failure.row >= 0 ? `Row ${failure.row + 1}: ` : ""}
          {failure.message}
        </span>
      )}
      {notice && !failure && <span className="truncate text-success">{notice}</span>}
      <span className="ml-auto truncate font-mono text-[11px] text-subtle">{result.sql.replace(/\s+/g, " ")}</span>
    </div>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <div className="flex h-full items-center justify-center text-[12px] text-subtle">{children}</div>;
}
