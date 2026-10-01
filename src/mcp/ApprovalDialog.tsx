import { Check, ChevronLeft, ChevronRight, Clock, ShieldCheck, TriangleAlert } from "lucide-react";
import { useEffect, useLayoutEffect, useReducer, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useKeymapHold } from "../commands/keymapHold";
import { useDataSources } from "../db/dataSources";
import { SqlViewer } from "../editor/SqlViewer";
import { EngineIcon } from "../ui/EngineIcon";
import { Modal } from "../ui/Modal";
import { Button, cx, IconButton } from "../ui/primitives";
import { useNow } from "../ui/useNow";
import type { ApprovalRequest } from "./api";
import { armed, armsIn, closedGate, currentRequest, rearm, showRequest } from "./approvalsQueue";
import { WARNING_LABEL, WRITE_KIND_LABEL } from "./labels";
import { clientInfoLabel, formatCountdown } from "./presence";
import { answerApproval, hideApprovals, stepApproval, useMcp } from "./store";

/** The countdown turns red this close to the end. */
const URGENT_MS = 15_000;

/**
 * Asks the user about a write an MCP client wants to run. It comes up over
 * everything, the window brought to the front by the app, also while the
 * user types in a console, so it is built not to be answered by accident:
 *
 * - Reject has the focus, and plain Enter never approves (Enter on Reject
 *   rejects; on Approve it does nothing).
 * - Approve is disabled for 800 ms after a request is shown, after moving
 *   to another one, and after the window comes back to the front.
 * - In those 800 ms Enter, Space and Escape do nothing at all, and any key
 *   restarts them, so someone still typing in a console answers nothing.
 * - Escape (and Later) puts the dialog aside without answering: the
 *   requests keep waiting until they expire, and the status bar's badge,
 *   or the next request, brings it back. A click outside does nothing.
 * - While it shows, it holds the keymap (see keymapHold.ts): no shortcut
 *   or menu command acts behind it, so ⌘⏎ meant for the console runs
 *   nothing.
 */
export function ApprovalDialog() {
  const approvals = useMcp((s) => s.approvals);
  const hidden = useMcp((s) => s.approvalsHidden);
  const current = currentRequest(approvals);
  if (!current || hidden) return null;
  return <ApprovalSheet request={current.request} index={current.index} total={approvals.requests.length} />;
}

function ApprovalSheet({ request, index, total }: { request: ApprovalRequest; index: number; total: number }) {
  useKeymapHold();
  const source = useDataSources((s) => s.sources.find((x) => x.id === request.dataSourceId));
  const now = useNow(1000);
  const [gate, setGate] = useState(closedGate);
  const [answering, setAnswering] = useState(false);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const rejectButton = useRef<HTMLButtonElement>(null);

  // Arm again, and give Reject the focus, whenever another request is shown.
  useLayoutEffect(() => {
    setGate((g) => showRequest(g, request.id, performance.now()));
    setAnswering(false);
    rejectButton.current?.focus();
  }, [request.id]);

  // Coming back to the window re-arms too: a click meant to focus it must not approve.
  useEffect(() => {
    const onFocus = () => setGate((g) => rearm(g, performance.now()));
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // Re-render once the gate opens.
  useEffect(() => {
    const wait = armsIn(gate, performance.now());
    if (wait === 0) return;
    const timer = window.setTimeout(rerender, wait + 16);
    return () => window.clearTimeout(timer);
  }, [gate]);

  const isArmed = () => armed(gate, performance.now());
  const left = Date.parse(request.expiresAt) - now;
  const expired = left <= 0;

  const answer = async (approve: boolean) => {
    if (answering) return;
    setAnswering(true);
    await answerApproval(request.id, approve);
    setAnswering(false);
  };

  const onKeyDownCapture = (e: KeyboardEvent) => {
    const early = !isArmed();
    // Someone still typing (in the console the dialog came up over) keeps
    // it from arming: each key restarts the wait.
    if (early && e.key !== "Tab") setGate((g) => rearm(g, performance.now()));
    const activates = e.key === "Enter" || e.key === " ";
    // Plain Enter never approves.
    const enterOnApprove = e.key === "Enter" && (e.target as Element).closest("[data-approve]");
    if ((early && activates) || enterOnApprove) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const reportedBy = clientInfoLabel(request.clientInfo?.name ?? null, request.clientInfo?.version ?? null);
  const color = request.dataSourceColor ?? source?.color ?? null;

  return (
    <Modal
      open
      onClose={hideApprovals}
      width={640}
      title={`${request.clientName} wants to run ${request.summary}`}
      description="It runs only if you approve it."
      aside={
        total > 1 && (
          <div className="flex shrink-0 items-center gap-0.5 text-[12px] text-muted tabular-nums">
            <IconButton label="Previous request" disabled={index === 0} onClick={() => stepApproval(-1)}>
              <ChevronLeft className="size-4" />
            </IconButton>
            <span aria-live="polite">
              {index + 1} of {total}
            </span>
            <IconButton label="Next request" disabled={index === total - 1} onClick={() => stepApproval(1)}>
              <ChevronRight className="size-4" />
            </IconButton>
          </div>
        )
      }
      contentProps={{
        onKeyDownCapture,
        // Escape too, while arming, would put aside a dialog nobody saw.
        onEscapeKeyDown: (e) => !isArmed() && e.preventDefault(),
        onInteractOutside: (e) => e.preventDefault(),
      }}
      footer={
        <>
          <span
            className={cx(
              "flex items-center gap-1.5 text-[12px] tabular-nums",
              expired || left < URGENT_MS ? "text-danger" : "text-muted",
            )}
          >
            <Clock className="size-3.5" />
            {expired ? "Expired" : `Expires in ${formatCountdown(left)}`}
          </span>
          <div className="ml-auto flex gap-2">
            <Button
              variant="ghost"
              title="Answer later: it keeps waiting until it expires (Esc)"
              onClick={hideApprovals}
            >
              Later
            </Button>
            <Button
              ref={rejectButton}
              autoFocus
              disabled={answering}
              // A key press (detail 0) counts only once armed; a click always does.
              onClick={(e) => (e.detail > 0 || isArmed()) && void answer(false)}
            >
              Reject
            </Button>
            <Button
              variant="primary"
              data-approve
              // Red when the statement destroys data or touches every row.
              className={cx(request.warnings.length > 0 && "bg-danger")}
              disabled={answering || expired || !isArmed()}
              onClick={() => isArmed() && void answer(true)}
            >
              <Check className="size-3.5" />
              Approve
            </Button>
          </div>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <dl className="grid grid-cols-[92px_1fr] items-baseline gap-x-3 gap-y-2 text-[12.5px]">
          <Row label="Client">
            <span className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
              <span className="font-medium text-fg">{request.clientName}</span>
              <span className="flex items-center gap-1 text-[11.5px] text-success" title="Its token is registered in IdeDB">
                <ShieldCheck className="size-3.5" /> Verified token
              </span>
            </span>
            {reportedBy && (
              <span className="mt-0.5 block text-[11.5px] text-subtle">
                Reported by the client, unverified: <span className="selectable text-muted">{reportedBy}</span>
              </span>
            )}
          </Row>
          <Row label="Data source">
            <span className="flex items-center gap-1.5">
              <span
                className="size-2 shrink-0 rounded-full border border-border-strong"
                style={color ? { background: color, borderColor: color } : undefined}
              />
              {source && <EngineIcon engine={source.params.engine} />}
              <span className="font-medium text-fg">{request.dataSourceName}</span>
            </span>
          </Row>
          <Row label="Statement">
            <span className="font-medium text-fg">{request.summary}</span>
            <span className="text-muted"> · {WRITE_KIND_LABEL[request.writeKind]}</span>
          </Row>
          {request.reason && (
            <Row label="Reason">
              <span className="selectable block text-fg">“{request.reason}”</span>
              <span className="mt-0.5 block text-[11.5px] text-subtle">Given by the client</span>
            </Row>
          )}
        </dl>

        {request.warnings.length > 0 && (
          <ul className="flex flex-col gap-1 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-[12px] text-danger">
            {request.warnings.map((warning) => (
              <li key={warning} className="flex items-start gap-2">
                <TriangleAlert className="mt-px size-3.5 shrink-0" />
                {WARNING_LABEL[warning]}
              </li>
            ))}
          </ul>
        )}

        <SqlViewer sql={request.sql} engine={source?.params.engine} label="Statement to approve" />
      </div>
    </Modal>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-right text-[12px] text-muted">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}
