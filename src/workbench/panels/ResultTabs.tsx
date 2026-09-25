import { LoaderCircle, Pin, X } from "lucide-react";
import { useConsoles, type ResultMeta } from "../../db/consoles";
import { cx } from "../../ui/primitives";

/**
 * Result tabs of a console. A run replaces the active tab unless it is
 * pinned; pinned tabs keep their result until closed.
 */
export function ResultTabs({
  consoleId,
  results,
  activeResultId,
}: {
  consoleId: string;
  results: ResultMeta[];
  activeResultId?: number;
}) {
  const { selectResult, togglePin, closeResult } = useConsoles.getState();

  return (
    <div role="tablist" aria-label="Results" className="flex h-8 shrink-0 items-stretch overflow-x-auto border-b border-border bg-bg">
      {results.map((result) => {
        const active = result.id === activeResultId;
        return (
          <div
            key={result.id}
            role="tab"
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            onMouseDown={() => selectResult(consoleId, result.id)}
            onAuxClick={(e) => e.button === 1 && void closeResult(consoleId, result.id)}
            className={cx(
              "group flex shrink-0 items-center gap-1.5 border-r border-border pr-1 pl-3 text-[12px]",
              active ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover hover:text-fg",
            )}
          >
            {result.status === "running" && <LoaderCircle className="size-3 animate-spin text-accent" />}
            {result.status === "error" && <span className="size-1.5 rounded-full bg-danger" />}
            <span className="whitespace-nowrap">{result.title}</span>
            <button
              type="button"
              aria-label={result.pinned ? "Unpin tab" : "Pin tab"}
              title={result.pinned ? "Unpin" : "Pin: keep this result when running again"}
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => togglePin(consoleId, result.id)}
              className={cx(
                "flex size-5 items-center justify-center rounded hover:bg-active",
                result.pinned ? "text-accent" : "text-subtle opacity-0 group-hover:opacity-100",
              )}
            >
              <Pin className={cx("size-3", result.pinned && "fill-current")} />
            </button>
            <button
              type="button"
              aria-label="Close tab"
              title="Close"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => void closeResult(consoleId, result.id)}
              className="flex size-5 items-center justify-center rounded text-subtle opacity-0 group-hover:opacity-100 hover:bg-active hover:text-fg"
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
