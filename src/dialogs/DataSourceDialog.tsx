import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { CircleAlert, CircleCheck, FolderOpen, LoaderCircle } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { api, errorMessage, type DataSource, type Engine, type SslMode } from "../db/api";
import { useDataSources } from "../db/dataSources";
import { EngineIcon, ENGINE_LABEL } from "../ui/EngineIcon";
import { Field, inputClass, Modal } from "../ui/Modal";
import { Button, cx } from "../ui/primitives";
import { passwordToSave, useDialogs } from "./dialogs";

const DEFAULT_PORT: Record<Engine, number | null> = { postgres: 5432, mysql: 3306, sqlite: null };

/** Accent colors, as in DataGrip, to tell environments apart at a glance. */
const COLORS = [null, "#30a46c", "#f5d90a", "#f76b15", "#e5484d", "#8e4ec6", "#3e63dd"];

type TestState =
  | { state: "idle" }
  | { state: "testing" }
  | { state: "ok"; message: string }
  | { state: "error"; message: string };

export function DataSourceDialog() {
  const initial = useDialogs((s) => s.dataSource);
  const close = useDialogs((s) => s.closeDataSource);
  return initial && <DataSourceForm key={initial.id || "new"} initial={initial} onClose={close} />;
}

function DataSourceForm({ initial, onClose }: { initial: DataSource; onClose: () => void }) {
  const save = useDataSources((s) => s.save);
  const [source, setSource] = useState(initial);
  /** `undefined` = leave the stored password untouched. */
  const [password, setPassword] = useState<string | undefined>(undefined);
  const [test, setTest] = useState<TestState>({ state: "idle" });
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string>();

  const isNew = !initial.id;
  const { params } = source;
  const isFile = params.engine === "sqlite";
  const setParams = (patch: Partial<DataSource["params"]>) => setSource((s) => ({ ...s, params: { ...s.params, ...patch } }));

  // Any edit invalidates a previous test result.
  useEffect(() => setTest({ state: "idle" }), [source, password]);

  const withDefaultName = (s: DataSource): DataSource => ({ ...s, name: s.name.trim() || suggestName(s) });

  const runTest = async () => {
    setTest({ state: "testing" });
    try {
      const { server, latencyMs } = await api.testDataSource(withDefaultName(source), password);
      setTest({ state: "ok", message: `${ENGINE_LABEL[server.engine]} ${server.version} · ${latencyMs} ms` });
    } catch (e) {
      setTest({ state: "error", message: errorMessage(e) });
    }
  };

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    setSaving(true);
    setSaveError(undefined);
    try {
      await save(withDefaultName(source), passwordToSave(isNew, source.savePassword, password));
      onClose();
    } catch (err) {
      setSaveError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const valid = isFile ? params.path.trim() !== "" : params.host.trim() !== "";

  return (
    <Modal
      open
      onClose={onClose}
      title={isNew ? "New Data Source" : `Edit ${initial.name}`}
      width={560}
      footer={
        <>
          <Button onClick={runTest} disabled={!valid || test.state === "testing"}>
            {test.state === "testing" && <LoaderCircle className="size-3.5 animate-spin" />}
            Test Connection
          </Button>
          <TestResult test={test} />
          <div className="ml-auto flex gap-2">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void submit()} disabled={!valid || saving}>
              {isNew ? "Create" : "Save"}
            </Button>
          </div>
        </>
      }
    >
      <form onSubmit={submit} className="flex flex-col gap-3">
        <div className="mb-1 grid grid-cols-3 gap-1 rounded-lg bg-inset p-1" role="radiogroup" aria-label="Engine">
          {(["postgres", "mysql", "sqlite"] as const).map((engine) => (
            <button
              key={engine}
              type="button"
              role="radio"
              aria-checked={params.engine === engine}
              onClick={() => setParams({ engine, port: params.port === DEFAULT_PORT[params.engine] ? null : params.port })}
              className={cx(
                "flex h-8 items-center justify-center gap-2 rounded-md text-[12.5px] transition-colors",
                params.engine === engine ? "bg-elevated text-fg shadow-sm" : "text-muted hover:text-fg",
              )}
            >
              <EngineIcon engine={engine} />
              {ENGINE_LABEL[engine]}
            </button>
          ))}
        </div>

        <Field label="Name">
          <input
            autoFocus
            className={inputClass}
            value={source.name}
            placeholder={suggestName(source)}
            onChange={(e) => setSource({ ...source, name: e.target.value })}
          />
        </Field>

        {isFile ? (
          <Field label="File">
            <div className="flex gap-2">
              <input
                className={cx(inputClass, "font-mono text-[12px]")}
                value={params.path}
                placeholder="/path/to/database.sqlite"
                spellCheck={false}
                onChange={(e) => setParams({ path: e.target.value })}
              />
              <Button
                onClick={async () => {
                  const picked = await openFileDialog({ multiple: false, directory: false });
                  if (typeof picked === "string") setParams({ path: picked });
                }}
              >
                <FolderOpen className="size-3.5" /> Browse…
              </Button>
            </div>
          </Field>
        ) : (
          <>
            <Field label="URL">
              <input
                className={cx(inputClass, "font-mono text-[12px]")}
                placeholder={`Paste a ${params.engine}:// URL to fill the fields below`}
                spellCheck={false}
                onChange={(e) => {
                  const parsed = parseUrl(e.target.value);
                  if (!parsed) return;
                  setSource((s) => ({ ...s, params: { ...s.params, ...parsed.params } }));
                  if (parsed.password !== undefined) setPassword(parsed.password);
                  e.target.value = "";
                }}
              />
            </Field>
            <Field label="Host">
              <div className="flex gap-2">
                <input
                  className={inputClass}
                  value={params.host}
                  placeholder="localhost"
                  spellCheck={false}
                  onChange={(e) => setParams({ host: e.target.value })}
                />
                <input
                  className={cx(inputClass, "w-20 shrink-0")}
                  value={params.port ?? ""}
                  placeholder={String(DEFAULT_PORT[params.engine])}
                  inputMode="numeric"
                  aria-label="Port"
                  onChange={(e) => {
                    const port = Number.parseInt(e.target.value, 10);
                    setParams({ port: Number.isFinite(port) && port > 0 && port < 65536 ? port : null });
                  }}
                />
              </div>
            </Field>
            <Field label="User">
              <input
                className={inputClass}
                value={params.user}
                spellCheck={false}
                onChange={(e) => setParams({ user: e.target.value })}
              />
            </Field>
            <Field label="Password">
              <div className="flex items-center gap-3">
                <input
                  type="password"
                  className={inputClass}
                  value={password ?? ""}
                  placeholder={isNew || !initial.savePassword ? "" : "<unchanged>"}
                  onChange={(e) => setPassword(e.target.value)}
                />
                <label className="flex shrink-0 items-center gap-1.5 text-[12px] text-muted">
                  <input
                    type="checkbox"
                    checked={source.savePassword}
                    onChange={(e) => setSource({ ...source, savePassword: e.target.checked })}
                    className="accent-accent"
                  />
                  Save in Keychain
                </label>
              </div>
            </Field>
            <Field label="Database">
              <input
                className={inputClass}
                value={params.database}
                placeholder={params.engine === "postgres" ? "postgres" : "optional"}
                spellCheck={false}
                onChange={(e) => setParams({ database: e.target.value })}
              />
            </Field>
            <Field label="SSL">
              <select
                className={cx(inputClass, "w-48")}
                value={params.sslMode}
                onChange={(e) => setParams({ sslMode: e.target.value as SslMode })}
              >
                <option value="disable">Disable</option>
                <option value="prefer">Prefer</option>
                <option value="require">Require</option>
                <option value="verify-full">Verify full</option>
              </select>
            </Field>
          </>
        )}

        <Field label="Color">
          <div className="flex gap-1.5">
            {COLORS.map((color) => (
              <button
                key={color ?? "none"}
                type="button"
                aria-label={color ? `Color ${color}` : "No color"}
                onClick={() => setSource({ ...source, color })}
                style={color ? { background: color } : undefined}
                className={cx(
                  "size-5 rounded-full border border-border-strong",
                  !color && "bg-[linear-gradient(135deg,transparent_45%,var(--fg-subtle)_45%,var(--fg-subtle)_55%,transparent_55%)]",
                  source.color === color && "ring-2 ring-accent ring-offset-2 ring-offset-[var(--bg-elevated)]",
                )}
              />
            ))}
          </div>
        </Field>

        {saveError && <p className="selectable text-[12px] text-danger">{saveError}</p>}
        {/* Enter anywhere in the form saves. */}
        <button type="submit" hidden disabled={!valid || saving} />
      </form>
    </Modal>
  );
}

function TestResult({ test }: { test: TestState }) {
  if (test.state === "ok") {
    return (
      <span className="flex min-w-0 items-center gap-1.5 text-[12px] text-success">
        <CircleCheck className="size-3.5 shrink-0" />
        <span className="truncate">{test.message}</span>
      </span>
    );
  }
  if (test.state === "error") {
    return (
      <span className="flex min-w-0 items-center gap-1.5 text-[12px] text-danger" title={test.message}>
        <CircleAlert className="size-3.5 shrink-0" />
        <span className="selectable truncate">{test.message}</span>
      </span>
    );
  }
  return null;
}

function suggestName({ params }: DataSource): string {
  if (params.engine === "sqlite") return params.path.split("/").pop() || "SQLite";
  const db = params.database || (params.engine === "postgres" ? "postgres" : "");
  const host = params.host || "localhost";
  return db ? `${db}@${host}` : host;
}

/** `postgres://user:pass@host:5432/db?sslmode=require` and `mysql://…` into fields. */
function parseUrl(text: string): { params: Partial<DataSource["params"]>; password?: string } | null {
  const trimmed = text.trim();
  const match = /^(postgres(?:ql)?|mysql):\/\//i.exec(trimmed);
  if (!match) return null;
  try {
    const url = new URL(trimmed);
    const engine: Engine = match[1].toLowerCase() === "mysql" ? "mysql" : "postgres";
    const ssl = url.searchParams.get("sslmode") ?? url.searchParams.get("ssl-mode");
    const sslMode = (["disable", "prefer", "require", "verify-full"] as const).find((m) => m === ssl?.toLowerCase());
    return {
      params: {
        engine,
        host: decodeURIComponent(url.hostname),
        port: url.port ? Number(url.port) : null,
        user: decodeURIComponent(url.username),
        database: decodeURIComponent(url.pathname.replace(/^\//, "")),
        ...(sslMode && { sslMode }),
      },
      password: url.password ? decodeURIComponent(url.password) : undefined,
    };
  } catch {
    return null;
  }
}
