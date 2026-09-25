import { Check, GitCommitVertical, Undo2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { executeCommand } from "../../commands/registry";
import { isRunning, useConsoles } from "../../db/consoles";
import { dismissNotice, formatElapsed, setMode, useTransactions, type TransactionMode } from "../../db/transactions";
import { cx, IconButton } from "../../ui/primitives";

const MODES: { mode: TransactionMode; label: string; hint: string }[] = [
  { mode: "auto", label: "Auto", hint: "Each statement commits on its own (unless you type BEGIN)" },
  { mode: "manual", label: "Manual", hint: "Statements and data editor changes stay uncommitted until Commit or Rollback" },
];

/** Ticks once a second while `active`, for the open transaction's clock. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

/** Console toolbar part for transactions: Tx Auto / Manual, the open transaction and Commit / Rollback. */
export function TransactionControls({ consoleId }: { consoleId: string }) {
  const mode = useTransactions((s) => s.modes[consoleId] ?? "auto");
  const open = useTransactions((s) => s.open[consoleId]);
  const notice = useTransactions((s) => s.notices[consoleId]);
  const ending = useTransactions((s) => !!s.ending[consoleId]);
  const inTransaction = useConsoles((s) => !!s.consoles[consoleId]?.inTransaction);
  const running = useConsoles((s) => isRunning(s.consoles[consoleId]));
  const now = useNow(inTransaction);
  const busy = running || ending;

  return (
    <>
      <div className="ml-2 flex shrink-0 items-center rounded-md bg-inset p-0.5 text-[11px]" role="radiogroup" aria-label="Transaction mode">
        <span className="px-1.5 text-subtle">Tx</span>
        {MODES.map(({ mode: m, label, hint }) => (
          <button
            key={m}
            type="button"
            role="radio"
            aria-checked={mode === m}
            title={hint}
            onClick={() => setMode(consoleId, m)}
            className={cx(
              "h-5 rounded px-1.5 font-medium transition-colors",
              mode === m ? "bg-elevated text-fg shadow-sm" : "text-muted hover:text-fg",
            )}
          >
            {label}
          </button>
        ))}
      </div>

      {inTransaction && (
        <>
          <span
            className={cx(
              "ml-2 flex shrink-0 items-center gap-1.5 rounded-md px-2 py-0.5 text-[11.5px] font-medium tabular-nums",
              open?.failed ? "bg-danger/15 text-danger" : "bg-warning/15 text-warning",
            )}
            title={
              open?.failed
                ? "A statement failed inside the transaction: PostgreSQL only accepts ROLLBACK now"
                : "Statements and data editor changes stay uncommitted until you commit or roll back"
            }
          >
            <GitCommitVertical className="size-3.5" />
            {open?.failed ? "Transaction failed" : "Transaction open"}
            {open && (
              <span className="font-normal opacity-80">
                · {open.statements} {open.statements === 1 ? "statement" : "statements"} · {formatElapsed(now - open.since)}
              </span>
            )}
          </span>
          <IconButton
            label="Commit"
            shortcut="$mod+Alt+Enter"
            disabled={busy}
            onClick={() => executeCommand("transaction.commit")}
            className="text-success hover:text-success"
          >
            <Check className="size-4" />
          </IconButton>
          <IconButton
            label="Rollback"
            shortcut="$mod+Alt+Shift+KeyZ"
            disabled={busy}
            onClick={() => executeCommand("transaction.rollback")}
            className="text-danger hover:text-danger"
          >
            <Undo2 className="size-4" />
          </IconButton>
        </>
      )}

      {notice && (
        <span className="ml-2 flex min-w-0 items-center gap-1 rounded-md bg-danger/10 py-0.5 pr-0.5 pl-2 text-[11.5px] text-danger">
          <span className="selectable truncate" title={notice}>
            {notice}
          </span>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => dismissNotice(consoleId)}
            className="flex size-4 shrink-0 items-center justify-center rounded hover:bg-danger/15"
          >
            <X className="size-3" />
          </button>
        </span>
      )}
    </>
  );
}
