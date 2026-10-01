import { CircleAlert, MousePointerClick, ShieldCheck, SquareTerminal } from "lucide-react";
import type { ReactNode } from "react";
import { newConsole } from "../actions";
import type { DataSource } from "../db/api";
import { useDataSources } from "../db/dataSources";
import { SqlViewer } from "../editor/SqlViewer";
import { EngineIcon } from "../ui/EngineIcon";
import { formatDuration } from "../ui/format";
import { Notice } from "../ui/Notice";
import { Button, cx } from "../ui/primitives";
import { useNow } from "../ui/useNow";
import { consoleBlocker, countLabel, formatFullTime, formatLogTime } from "./activity";
import type { AuditEntry, Decision } from "./api";
import { DECISION_HINT, DECISION_LABEL, DECISION_TONE, TRANSPORT_LABEL } from "./labels";
import { clientInfoLabel } from "./presence";
import { CopyButton } from "./SnippetPicker";

const TONE_CLASS: Record<(typeof DECISION_TONE)[Decision], string> = {
  neutral: "bg-active text-muted",
  success: "bg-success/15 text-success",
  danger: "bg-danger/15 text-danger",
  warning: "bg-warning/15 text-warning",
};

/** A decision as a small pill: quiet for what just ran, colored for everything else. */
export function DecisionBadge({ decision, className }: { decision: Decision; className?: string }) {
  return (
    <span
      className={cx(
        "inline-flex h-[18px] shrink-0 items-center rounded-full px-1.5 text-[11px] font-medium",
        TONE_CLASS[DECISION_TONE[decision]],
        className,
      )}
    >
      {DECISION_LABEL[decision]}
    </span>
  );
}

/** A data source's color, hollow when it has none or no longer exists. */
export function SourceDot({ source }: { source: DataSource | undefined }) {
  const color = source?.color;
  return (
    <span
      aria-hidden
      className="size-2 shrink-0 rounded-full border border-border-strong"
      style={color ? { background: color, borderColor: color } : undefined}
    />
  );
}

/** Opens a new console on the row's data source with its SQL in the editor, not run. */
export function openInConsole(entry: AuditEntry) {
  const exists = useDataSources.getState().sources.some((s) => s.id === entry.dataSourceId);
  if (consoleBlocker(entry, exists) !== null) return;
  newConsole(entry.dataSourceId!, entry.sql!);
}

/** Everything recorded about one tool call, its SQL, and what can be done with it. */
export function ActivityDetail({ entry }: { entry: AuditEntry | undefined }) {
  const source = useDataSources((s) => (entry ? s.sources.find((x) => x.id === entry.dataSourceId) : undefined));
  const now = useNow(60_000);

  if (!entry) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-[12px] text-subtle">
        <MousePointerClick className="size-5" strokeWidth={1.5} />
        Select an entry to see everything recorded about it.
      </div>
    );
  }

  const blocker = consoleBlocker(entry, !!source);
  const reportedAs = clientInfoLabel(entry.clientInfoName, entry.clientInfoVersion);
  const title = `${entry.statementKind ?? entry.tool}${entry.dataSourceName ? ` on ${entry.dataSourceName}` : ""}`;

  return (
    <section aria-label="Activity details" className="flex h-full min-w-0 flex-col">
      <header className="flex shrink-0 items-start gap-2 border-b border-border px-4 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <DecisionBadge decision={entry.decision} />
            <h2 className="selectable truncate text-[13px] font-semibold text-fg" title={title}>
              {title}
            </h2>
          </div>
          <p className="mt-0.5 truncate text-[11.5px] text-subtle">
            {entry.clientName} · <span title={formatFullTime(entry.at)}>{formatLogTime(entry.at, now)}</span>
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {entry.sql !== null && <CopyButton text={entry.sql} label="Copy SQL" />}
          {/* A disabled button shows no tooltip; its wrapper does. */}
          <span title={blocker ?? "A new console on the data source, with this SQL in the editor, not run  ⏎"}>
            <Button disabled={blocker !== null} onClick={() => openInConsole(entry)}>
              <SquareTerminal className="size-3.5" />
              Open in Console
            </Button>
          </span>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3">
        {entry.error && (
          <Notice tone="danger" icon={CircleAlert}>
            <span className="selectable min-w-0 [overflow-wrap:anywhere]">{entry.error}</span>
          </Notice>
        )}
        {entry.sql !== null && (
          <div className="flex flex-col gap-1">
            <SqlViewer sql={entry.sql} engine={source?.params.engine} maxHeight={220} label="SQL" />
            {entry.sqlTruncated && (
              <p className="text-[11.5px] text-warning">The SQL was longer: only its first 100 KiB was recorded.</p>
            )}
          </div>
        )}

        <dl className="grid grid-cols-[112px_minmax(0,1fr)] items-baseline gap-x-3 gap-y-1.5 text-[12px]">
          <Field label="Time">
            <Value>{formatFullTime(entry.at)}</Value>
          </Field>
          <Field label="Decision">
            <span className="text-fg">{DECISION_LABEL[entry.decision]}</span>
            <span className="text-muted"> · {DECISION_HINT[entry.decision]}</span>
          </Field>
          <Field label="Client">
            <span className="flex flex-wrap items-center gap-x-2">
              <span className="font-medium text-fg">{entry.clientName}</span>
              <span className="flex items-center gap-1 text-[11.5px] text-success" title="Its token is registered in IdeDB">
                <ShieldCheck className="size-3.5" /> Verified token
              </span>
            </span>
          </Field>
          <Field label="Reported as">
            {reportedAs ? (
              <>
                <span className="selectable text-fg">{reportedAs}</span>
                <span className="text-subtle"> · unverified, as the client says</span>
              </>
            ) : (
              <None />
            )}
          </Field>
          <Field label="Data source">
            {entry.dataSourceName ? (
              <span className="flex min-w-0 items-center gap-1.5">
                <SourceDot source={source} />
                {source && <EngineIcon engine={source.params.engine} />}
                <span className={cx("truncate", source ? "text-fg" : "text-muted")}>{entry.dataSourceName}</span>
                {!source && <span className="shrink-0 text-subtle">· no longer exists</span>}
              </span>
            ) : (
              <None />
            )}
          </Field>
          <Field label="Tool">
            <span className="font-mono text-[11.5px] text-fg">{entry.tool}</span>
          </Field>
          <Field label="Statement">{entry.statementKind ? <span className="text-fg">{entry.statementKind}</span> : <None />}</Field>
          {entry.reason !== null && (
            <Field label="Reason">
              <span className="selectable text-fg">“{entry.reason}”</span>
              <span className="text-subtle"> · given by the client</span>
            </Field>
          )}
          <Field label="Approval wait">
            {entry.approvalWaitMs !== null ? <Value>{formatDuration(entry.approvalWaitMs)}</Value> : <None />}
          </Field>
          <Field label="Elapsed">{entry.elapsedMs !== null ? <Value>{formatDuration(entry.elapsedMs)}</Value> : <None />}</Field>
          <Field label="Rows">
            {entry.rowCount !== null ? <Value>{countLabel({ ...entry, truncated: false })}</Value> : <None />}
            {entry.truncated && <span className="text-warning"> · result cut short</span>}
          </Field>
          <Field label="SQL truncated">
            <Value>{entry.sqlTruncated ? "Yes, to its first 100 KiB" : "No"}</Value>
          </Field>
          <Field label="Transport">
            <Value>{TRANSPORT_LABEL[entry.transport]}</Value>
          </Field>
          <Field label="Protocol">{entry.protocolVersion ? <Value>{entry.protocolVersion}</Value> : <None />}</Field>
          <Field label="Session">
            {entry.sessionKey ? (
              <span className="selectable font-mono text-[11.5px] break-all text-fg">{entry.sessionKey}</span>
            ) : (
              <None />
            )}
          </Field>
          <Field label="Entry">
            <Value>#{entry.id}</Value>
          </Field>
        </dl>
      </div>
    </section>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-right text-muted">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}

function Value({ children }: { children: ReactNode }) {
  return <span className="selectable text-fg tabular-nums">{children}</span>;
}

function None() {
  return <span className="text-subtle">—</span>;
}
