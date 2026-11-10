import { CircleAlert, LoaderCircle, ShieldAlert } from "lucide-react";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { errorMessage } from "../db/api";
import { inputClass } from "../ui/Modal";
import { Button, cx, StatusDot, Switch } from "../ui/primitives";
import type { McpSettings } from "./api";
import { draftChanged, draftOf, parseDraft, type SettingsField } from "./settingsForm";
import { endpointUrl } from "./snippets";
import { CopyButton } from "./SnippetPicker";
import { serverState } from "./status";
import { retryServer, saveSettings, toggleServer, useMcp } from "./store";
import { useT, type MessageKey } from "../i18n";

/** The server: on or off, where it listens, and the limits every client gets. */
export function ServerTab() {
  const t = useT();
  const settings = useMcp((s) => s.settings);
  const loadError = useMcp((s) => s.loadError);
  if (!settings) {
    return (
      <div className="flex flex-1 items-center justify-center p-6 text-[12px] text-muted">
        {loadError ? <span className="selectable text-danger">{loadError}</span> : <LoaderCircle className="size-4 animate-spin" />}
      </div>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="flex max-w-[680px] flex-col gap-4 px-4 py-3">
        <StatusCard settings={settings} />
        <SettingsForm settings={settings} />
        <div className="flex items-start gap-2 rounded-md bg-inset px-3 py-2 text-[12px] text-muted">
          <ShieldAlert className="mt-px size-3.5 shrink-0 text-warning" />
          <p>{t("mcp.server.warning")}</p>
        </div>
      </div>
    </div>
  );
}

/** On/off, applied at once, and what the server is doing. */
function StatusCard({ settings }: { settings: McpSettings }) {
  const t = useT();
  const status = useMcp((s) => s.status);
  const [busy, setBusy] = useState(false);
  const { tone, label } = serverState(status, settings.enabled);
  const url = status.url ?? endpointUrl(settings.port);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    await action();
    setBusy(false);
  };

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border px-4 py-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium text-fg">{t("mcp.server.title")}</div>
          <div className="mt-0.5 flex items-center gap-1.5 text-[12px] text-muted">
            <StatusDot tone={tone} />
            <span className={cx(tone === "danger" && "text-danger")}>{label}</span>
          </div>
        </div>
        {status.error && settings.enabled && (
          <Button disabled={busy} onClick={() => void run(retryServer)}>
            {t("mcp.banner.retry")}
          </Button>
        )}
        <Switch
          label={t("mcp.server.title")}
          checked={settings.enabled}
          disabled={busy}
          onCheckedChange={() => void run(toggleServer)}
        />
      </div>
      {status.error && (
        <p className="selectable flex items-start gap-1.5 text-[12px] text-danger">
          <CircleAlert className="mt-px size-3.5 shrink-0" />
          {status.error}
        </p>
      )}
      <div className="flex items-center gap-2 border-t border-border pt-3">
        <span className="w-[136px] shrink-0 text-right text-[12px] text-muted">{t("mcp.server.endpoint")}</span>
        <code className={cx("selectable min-w-0 truncate font-mono text-[12px]", status.running ? "text-fg" : "text-muted")}>
          {url}
        </code>
        <CopyButton text={url} label={t("mcp.server.copyEndpoint")} />
      </div>
    </div>
  );
}

const FIELDS: { field: SettingsField; label: MessageKey; unit?: MessageKey; hint: MessageKey }[] = [
  { field: "port", label: "mcp.server.port", hint: "mcp.server.port.hint" },
  { field: "maxRows", label: "mcp.server.maxRows", unit: "mcp.server.rows", hint: "mcp.server.maxRows.hint" },
  { field: "statementTimeoutSecs", label: "mcp.server.statementTimeout", unit: "mcp.server.seconds", hint: "mcp.server.statementTimeout.hint" },
  { field: "writeTimeoutSecs", label: "mcp.server.writeTimeout", unit: "mcp.server.seconds", hint: "mcp.server.writeTimeout.hint" },
  { field: "approvalTimeoutSecs", label: "mcp.server.approvalTimeout", unit: "mcp.server.seconds", hint: "mcp.server.approvalTimeout.hint" },
];

/** Port and limits, saved together. A new port restarts the server. */
function SettingsForm({ settings }: { settings: McpSettings }) {
  const t = useT();
  // The draft and the settings it started from: unedited, it follows them.
  const [form, setForm] = useState(() => ({ base: settings, draft: draftOf(settings) }));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const dirty = draftChanged(form.draft, form.base);

  useEffect(() => {
    setForm((f) => (draftChanged(f.draft, f.base) ? f : { base: settings, draft: draftOf(settings) }));
  }, [settings]);

  const { settings: parsed, errors } = parseDraft(form.draft, settings);
  const set = (field: SettingsField, value: string) => setForm((f) => ({ ...f, draft: { ...f.draft, [field]: value } }));

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!parsed || !dirty || saving) return;
    setSaving(true);
    setError(undefined);
    try {
      await saveSettings(parsed);
      setForm({ base: parsed, draft: draftOf(parsed) });
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const revert = () => {
    setForm({ base: settings, draft: draftOf(settings) });
    setError(undefined);
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <div className="flex flex-col gap-2.5">
        {FIELDS.map(({ field, label, unit, hint }, i) => (
          <SettingRow
            key={field}
            label={t(label)}
            hint={errors[field] ?? t(hint)}
            invalid={!!errors[field]}
            separated={i === 1}
          >
            <input
              inputMode="numeric"
              aria-label={t(label)}
              aria-invalid={!!errors[field]}
              className={cx(inputClass, "w-24 tabular-nums", errors[field] && "border-danger focus:border-danger")}
              value={form.draft[field]}
              onChange={(e) => set(field, e.target.value)}
            />
            {unit && <span className="text-[12px] text-subtle">{t(unit)}</span>}
          </SettingRow>
        ))}
      </div>
      {error && <p className="selectable pl-[148px] text-[12px] text-danger">{error}</p>}
      <div className="flex gap-2 pl-[148px]">
        <Button type="submit" variant="primary" disabled={!dirty || !parsed || saving}>
          {t("common.save")}
        </Button>
        <Button variant="ghost" disabled={!dirty || saving} onClick={revert}>
          {t("mcp.server.revert")}
        </Button>
        {dirty && form.draft.port.trim() !== String(settings.port) && settings.enabled && (
          <span className="self-center text-[11.5px] text-subtle">{t("mcp.server.restarts")}</span>
        )}
      </div>
    </form>
  );
}

function SettingRow({
  label,
  hint,
  invalid,
  separated,
  children,
}: {
  label: string;
  hint: string;
  invalid: boolean;
  /** Starts the limits, under a heading. */
  separated?: boolean;
  children: ReactNode;
}) {
  const t = useT();
  return (
    <>
      {separated && <div className="mt-1 pl-[148px] text-[12px] font-semibold text-fg">{t("mcp.server.limits")}</div>}
      <label className="grid grid-cols-[136px_1fr] items-start gap-3">
        <span className="pt-1.5 text-right text-[12px] text-muted">{label}</span>
        <span className="flex flex-col gap-1">
          <span className="flex items-center gap-2">{children}</span>
          <span className={cx("text-[11.5px]", invalid ? "text-danger" : "text-subtle")}>{hint}</span>
        </span>
      </label>
    </>
  );
}
