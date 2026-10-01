import { CircleAlert, Info, KeyRound, Power } from "lucide-react";
import { useState, type FormEvent } from "react";
import { errorMessage } from "../db/api";
import { inputClass, Modal } from "../ui/Modal";
import { Notice } from "../ui/Notice";
import { Button, cx } from "../ui/primitives";
import { showMcp } from "../workbench/Workbench";
import type { McpClient } from "./api";
import { CopyButton, SnippetPicker } from "./SnippetPicker";
import { closeDialog, createClient, renameClient, toggleServer, useMcp } from "./store";

/** Names to start from, matching the snippets. */
const SUGGESTED_NAMES = ["claude-code", "cursor"];

/** The MCP dialogs: a new client, its token (shown once) and renaming one. */
export function McpDialogs() {
  const dialog = useMcp((s) => s.dialog);
  if (!dialog) return null;
  switch (dialog.kind) {
    case "newClient":
      return <NewClientDialog />;
    case "token":
      return (
        <TokenDialog
          key={dialog.token}
          client={dialog.client}
          token={dialog.token}
          regenerated={dialog.regenerated}
        />
      );
    case "rename":
      return <RenameDialog key={dialog.client.id} client={dialog.client} />;
  }
}

function NewClientDialog() {
  const clients = useMcp((s) => s.clients);
  const taken = new Set(clients.map((c) => c.name));
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string>();
  const valid = name.trim() !== "";

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!valid || creating) return;
    setCreating(true);
    setError(undefined);
    try {
      await createClient(name.trim());
    } catch (err) {
      setError(errorMessage(err));
      setCreating(false);
    }
  };

  return (
    <Modal
      open
      onClose={closeDialog}
      title="New MCP Client"
      description="One for each app or agent that should reach your data sources through IdeDB, each with its own token."
      width={460}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="ghost" onClick={closeDialog}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid || creating} onClick={() => void submit()}>
            Create
          </Button>
        </div>
      }
    >
      <form onSubmit={submit} className="flex flex-col gap-2.5">
        <input
          autoFocus
          aria-label="Client name"
          placeholder="Name, e.g. claude-code"
          spellCheck={false}
          className={inputClass}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <div className="flex items-center gap-1.5">
          {SUGGESTED_NAMES.filter((suggestion) => !taken.has(suggestion)).map((suggestion) => (
            <button
              key={suggestion}
              type="button"
              onClick={() => setName(suggestion)}
              className={cx(
                "h-6 rounded-md border px-2 font-mono text-[11.5px] transition-colors",
                name === suggestion
                  ? "border-accent bg-accent-soft text-fg"
                  : "border-border text-muted hover:border-border-strong hover:text-fg",
              )}
            >
              {suggestion}
            </button>
          ))}
        </div>
        <p className="text-[11.5px] text-subtle">
          Every statement it runs is recorded under this name. It starts with no access to any data source.
        </p>
        {error && <p className="selectable text-[12px] text-danger">{error}</p>}
      </form>
    </Modal>
  );
}

/** A token just created or regenerated, with what to paste into the client. Shown once. */
function TokenDialog({ client, token, regenerated }: { client: McpClient; token: string; regenerated: boolean }) {
  const settings = useMcp((s) => s.settings);
  const status = useMcp((s) => s.status);
  const port = status.port ?? settings?.port ?? 7412;
  const serverOff = !status.running;

  const done = () => {
    closeDialog();
    // Where its access is granted.
    if (!regenerated) showMcp("clients");
  };

  return (
    <Modal
      open
      onClose={done}
      title={regenerated ? `New Token for ${client.name}` : `Token for ${client.name}`}
      description="Copy the token now: IdeDB keeps only a hash of it and can't show it again."
      width={600}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="primary" autoFocus onClick={done}>
            Done
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex items-center gap-2 rounded-md border border-border-strong bg-inset py-1 pr-1 pl-3">
          <KeyRound className="size-3.5 shrink-0 text-warning" />
          <code className="selectable min-w-0 flex-1 truncate font-mono text-[12px] text-fg" title={token}>
            {token}
          </code>
          <CopyButton text={token} label="Copy token" />
        </div>

        <section className="flex flex-col gap-1.5">
          <h3 className="text-[12px] font-medium text-fg">Connect {client.name}</h3>
          <SnippetPicker port={port} token={token} />
        </section>

        {serverOff && (
          <Notice tone={status.error ? "danger" : "muted"} icon={status.error ? CircleAlert : Power}>
            <span className="flex-1">
              {status.error ? `The MCP server couldn't start: ${status.error}` : "The MCP server is off, so the client can't connect yet."}
            </span>
            {!status.error && (
              <Button className="h-6" onClick={() => void toggleServer()}>
                Turn On
              </Button>
            )}
          </Notice>
        )}
        {!regenerated && client.grants.length === 0 && (
          <Notice tone="muted" icon={Info}>
            It can't see any data source yet: grant access in the MCP tool window, which opens when you close this.
          </Notice>
        )}
      </div>
    </Modal>
  );
}

function RenameDialog({ client }: { client: McpClient }) {
  const [name, setName] = useState(client.name);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const valid = name.trim() !== "";

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!valid || saving) return;
    setSaving(true);
    try {
      await renameClient(client.id, name.trim());
      closeDialog();
    } catch (err) {
      setError(errorMessage(err));
      setSaving(false);
    }
  };

  return (
    <Modal
      open
      onClose={closeDialog}
      title="Rename Client"
      description="Its activity keeps the name it ran under."
      width={400}
      footer={
        <div className="ml-auto flex gap-2">
          <Button variant="ghost" onClick={closeDialog}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!valid || saving} onClick={() => void submit()}>
            Rename
          </Button>
        </div>
      }
    >
      <form onSubmit={submit} className="flex flex-col gap-2">
        <input
          autoFocus
          aria-label="Client name"
          spellCheck={false}
          className={inputClass}
          value={name}
          onFocus={(e) => e.currentTarget.select()}
          onChange={(e) => setName(e.target.value)}
        />
        {error && <p className="selectable text-[12px] text-danger">{error}</p>}
      </form>
    </Modal>
  );
}
