import { Ban, Bot, CircleAlert, Lock, LockOpen, Pencil, Plus, Power, RotateCw, Trash2 } from "lucide-react";
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { editDataSource } from "../actions";
import { executeCommand } from "../commands/registry";
import type { DataSource } from "../db/api";
import { useDataSources } from "../db/dataSources";
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "../ui/ContextMenu";
import { ENGINE_LABEL, EngineIcon } from "../ui/EngineIcon";
import { Notice } from "../ui/Notice";
import { Button, cx, IconButton, Segmented, StatusDot } from "../ui/primitives";
import { useNow } from "../ui/useNow";
import type { McpClient } from "./api";
import { grantLevel, levelBlocked, unavailableReason, type GrantLevel } from "./grants";
import { clientInfoLabel, formatDay, isOnline, presenceLabel, presenceShort } from "./presence";
import { SnippetPicker } from "./SnippetPicker";
import { TOKEN_PLACEHOLDER } from "./snippets";
import {
  deleteClient,
  loadMcp,
  openNewClient,
  openRename,
  regenerateToken,
  revokeClient,
  selectClient,
  setAccess,
  setMcpTab,
  setNeverWrite,
  toggleServer,
  useMcp,
} from "./store";

/** How often presence ("Online", "5 min ago") is worked out again. */
const PRESENCE_TICK_MS = 15_000;

const NEVER_WRITE_HINT =
  "Never write: refuses every write on this data source without asking, for every MCP client, whatever their access.";

/** The MCP clients, and what the selected one may do on each data source. */
export function ClientsTab() {
  const clients = useMcp((s) => s.clients);
  const loaded = useMcp((s) => s.loaded);
  const loadError = useMcp((s) => s.loadError);
  const selectedId = useMcp((s) => s.selectedClientId);
  const now = useNow(PRESENCE_TICK_MS);
  const selected = clients.find((c) => c.id === selectedId) ?? clients[0];

  let body: ReactNode = null;
  if (loaded && clients.length === 0) {
    body = (
      <EmptyState icon={Bot} title="No MCP clients yet">
        Create one for each app that should reach your data sources through IdeDB, like Claude Code or Cursor. Each
        gets its own token, and only the access you grant it.
        <Button variant="primary" className="mt-3" onClick={openNewClient}>
          <Plus className="size-3.5" /> New Client
        </Button>
      </EmptyState>
    );
  } else if (loaded) {
    body = (
      <div className="flex min-h-0 flex-1">
        <ClientList clients={clients} selected={selected} now={now} />
        {selected && <ClientDetail client={selected} now={now} />}
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {loadError && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-danger/10 px-3 py-1.5 text-[12px] text-danger">
          <CircleAlert className="size-3.5 shrink-0" />
          <span className="selectable min-w-0 flex-1 truncate" title={loadError}>
            Couldn't load the MCP server's state: {loadError}
          </span>
          <Button className="h-6" onClick={() => void loadMcp()}>
            Retry
          </Button>
        </div>
      )}
      <ServerBanner />
      {body}
    </div>
  );
}

/** Says when clients can't connect: the server is off or failed to start. */
function ServerBanner() {
  const status = useMcp((s) => s.status);
  const enabled = useMcp((s) => s.settings?.enabled ?? false);
  const loaded = useMcp((s) => s.loaded);
  if (!loaded || status.running || (enabled && !status.error)) return null;
  const failed = !!status.error;
  return (
    <div
      className={cx(
        "flex shrink-0 items-center gap-2 border-b border-border px-3 py-1.5 text-[12px]",
        failed ? "bg-danger/10 text-danger" : "bg-inset text-muted",
      )}
    >
      {failed ? <CircleAlert className="size-3.5 shrink-0" /> : <Power className="size-3.5 shrink-0" />}
      <span className="selectable min-w-0 flex-1 truncate" title={status.error ?? undefined}>
        {failed
          ? `The MCP server couldn't start: ${status.error}`
          : "The MCP server is off, so no client can connect."}
      </span>
      {failed ? (
        <Button className="h-6" onClick={() => setMcpTab("server")}>
          Server Settings
        </Button>
      ) : (
        <Button className="h-6" onClick={() => void toggleServer()}>
          Turn On
        </Button>
      )}
    </div>
  );
}

function EmptyState({ icon: Icon, title, children }: { icon: typeof Bot; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 overflow-y-auto p-6 text-center">
      <Icon className="size-6 text-subtle" strokeWidth={1.5} />
      <p className="text-[13px] font-medium text-fg">{title}</p>
      <div className="flex max-w-[420px] flex-col items-center text-[12px] text-muted">{children}</div>
    </div>
  );
}

function ClientList({ clients, selected, now }: { clients: McpClient[]; selected?: McpClient; now: number }) {
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!selected) return;
    list.current?.querySelector(`[data-client="${selected.id}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const onKeyDown = (e: KeyboardEvent) => {
    const index = clients.findIndex((c) => c.id === selected?.id);
    const go = (i: number) => {
      e.preventDefault();
      selectClient(clients[Math.max(0, Math.min(clients.length - 1, i))].id);
    };
    if (e.key === "ArrowDown") go(index + 1);
    else if (e.key === "ArrowUp") go(index - 1);
    else if (e.key === "Home") go(0);
    else if (e.key === "End") go(clients.length - 1);
    else if (selected && e.key === "F6" && e.shiftKey) {
      e.preventDefault();
      openRename(selected);
    } else if (selected && (e.key === "Backspace" || e.key === "Delete")) {
      e.preventDefault();
      void deleteClient(selected);
    }
  };

  return (
    <div className="flex w-[clamp(11rem,32%,16rem)] shrink-0 flex-col border-r border-border">
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-border px-1.5">
        <IconButton label="New Client" onClick={openNewClient}>
          <Plus className="size-4" />
        </IconButton>
        <span className="ml-auto px-1.5 text-[11px] text-subtle tabular-nums">
          {clients.length} {clients.length === 1 ? "client" : "clients"}
        </span>
      </div>
      <div
        ref={list}
        role="listbox"
        aria-label="MCP clients"
        tabIndex={0}
        aria-activedescendant={selected ? `mcp-client-${selected.id}` : undefined}
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-y-auto py-1 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
      >
        {clients.map((client) => (
          <ClientRow key={client.id} client={client} selected={client.id === selected?.id} now={now} />
        ))}
      </div>
    </div>
  );
}

function ClientRow({ client, selected, now }: { client: McpClient; selected: boolean; now: number }) {
  const revoked = !!client.revokedAt;
  const online = !revoked && isOnline(client.lastSeenAt, now);
  const info = clientInfoLabel(client.lastClientName, client.lastClientVersion);

  return (
    <ContextMenuRoot>
      <ContextMenuTrigger asChild>
        <div
          id={`mcp-client-${client.id}`}
          data-client={client.id}
          role="option"
          aria-selected={selected}
          onMouseDown={() => selectClient(client.id)}
          onContextMenu={() => selectClient(client.id)}
          onDoubleClick={() => openRename(client)}
          className={cx(
            "mx-1 flex items-center gap-2 rounded-md px-2 py-1.5",
            selected ? "bg-accent-soft" : "hover:bg-hover",
          )}
        >
          {revoked ? (
            <Ban className="size-2.5 shrink-0 text-danger" />
          ) : (
            <StatusDot tone={online ? "success" : "idle"} />
          )}
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline gap-2">
              <span className={cx("truncate text-[12.5px]", revoked ? "text-muted" : "text-fg")}>{client.name}</span>
              <span
                className={cx(
                  "ml-auto shrink-0 text-[11px]",
                  revoked ? "text-danger" : online ? "text-success" : "text-subtle",
                )}
                title={revoked ? undefined : presenceLabel(client.lastSeenAt, now)}
              >
                {revoked ? "Revoked" : presenceShort(client.lastSeenAt, now)}
              </span>
            </div>
            <div className="flex min-w-0 items-baseline gap-1.5 text-[11px] text-subtle">
              <span className="shrink-0 font-mono">{client.tokenPrefix}…</span>
              {info && (
                <span className="truncate" title={`${info}, as reported by the client`}>
                  · {info}
                </span>
              )}
            </div>
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem label="Rename…" onSelect={() => openRename(client)} />
        <ContextMenuItem label="Regenerate Token…" disabled={revoked} onSelect={() => void regenerateToken(client)} />
        <ContextMenuSeparator />
        <ContextMenuItem label="Revoke…" disabled={revoked} onSelect={() => void revokeClient(client)} />
        <ContextMenuItem label="Delete…" onSelect={() => void deleteClient(client)} />
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}

function ClientDetail({ client, now }: { client: McpClient; now: number }) {
  const port = useMcp((s) => s.status.port ?? s.settings?.port ?? null);
  const revoked = !!client.revokedAt;
  const online = !revoked && isOnline(client.lastSeenAt, now);
  const info = clientInfoLabel(client.lastClientName, client.lastClientVersion);

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <header className="flex shrink-0 items-start gap-3 border-b border-border px-4 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2">
            <h2 className="selectable truncate text-[13px] font-semibold text-fg">{client.name}</h2>
            <span className={cx("shrink-0 text-[11.5px]", revoked ? "text-danger" : online ? "text-success" : "text-subtle")}>
              {revoked ? "Revoked" : presenceLabel(client.lastSeenAt, now)}
            </span>
          </div>
          <p className="mt-0.5 flex flex-wrap gap-x-3 text-[11.5px] text-subtle">
            <span>
              Token <span className="font-mono text-muted">{client.tokenPrefix}…</span>
            </span>
            <span>Created {formatDay(client.createdAt, now)}</span>
            {info && (
              <span title="What the client said about itself. IdeDB only verifies the token.">
                Reported as <span className="text-muted">{info}</span>
              </span>
            )}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          <IconButton label="Rename…" onClick={() => openRename(client)}>
            <Pencil className="size-3.5" />
          </IconButton>
          <IconButton label="Regenerate Token…" disabled={revoked} onClick={() => void regenerateToken(client)}>
            <RotateCw className="size-3.5" />
          </IconButton>
          <IconButton label="Revoke…" disabled={revoked} onClick={() => void revokeClient(client)}>
            <Ban className="size-3.5" />
          </IconButton>
          <IconButton label="Delete…" className="hover:text-danger" onClick={() => void deleteClient(client)}>
            <Trash2 className="size-3.5" />
          </IconButton>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-3">
        {revoked && (
          <Notice tone="danger" icon={Ban}>
            Revoked {formatDay(client.revokedAt!, now)}: its token no longer works. Delete it to remove it from the
            list; its activity stays in the log.
          </Notice>
        )}
        <section className="flex flex-col gap-2">
          <SectionTitle
            title="Access"
            hint="Reads run in read-only sessions. Every write waits for your approval in IdeDB."
          />
          <GrantsMatrix client={client} />
        </section>
        {!revoked && port !== null && (
          <section className="flex flex-col gap-2">
            <SectionTitle
              title="Connect"
              hint={`Paste into the client with ${TOKEN_PLACEHOLDER} replaced by its token. Lost it? Regenerate it.`}
            />
            <SnippetPicker port={port} />
          </section>
        )}
      </div>
    </div>
  );
}

function SectionTitle({ title, hint }: { title: string; hint: string }) {
  return (
    <div>
      <h3 className="text-[12px] font-semibold text-fg">{title}</h3>
      <p className="text-[11.5px] text-subtle">{hint}</p>
    </div>
  );
}

const LEVEL_LABEL: Record<GrantLevel, string> = { none: "None", read: "Read", write: "Write" };
const LEVEL_TITLE: Record<GrantLevel, string> = {
  none: "No access: the client doesn't see this data source",
  read: "Read: queries in read-only sessions",
  write: "Write: reads, and writes you approve one by one",
};

/** One row per data source: the client's access, and the never-write lock for every client. */
function GrantsMatrix({ client }: { client: McpClient }) {
  const sources = useDataSources((s) => s.sources);
  const loaded = useDataSources((s) => s.loaded);
  const neverWrite = useMcp((s) => s.neverWrite);

  if (loaded && sources.length === 0) {
    return (
      <div className="flex flex-col items-center gap-2 rounded-md border border-dashed border-border-strong px-4 py-5 text-center">
        <p className="text-[12px] text-muted">No data sources yet. Add one, then choose what this client may do on it.</p>
        <Button onClick={() => executeCommand("datasource.new")}>
          <Plus className="size-3.5" /> New Data Source
        </Button>
      </div>
    );
  }

  return (
    <div role="table" aria-label={`Access of ${client.name}`} className="rounded-md border border-border">
      <div
        role="row"
        className="grid h-7 grid-cols-[minmax(0,1fr)_auto_64px] items-center gap-3 border-b border-border px-3 text-[11px] text-subtle"
      >
        <span role="columnheader">Data source</span>
        <span role="columnheader" className="w-[138px]">
          Access
        </span>
        <span role="columnheader" className="text-center" title={NEVER_WRITE_HINT}>
          Never write
        </span>
      </div>
      {sources.map((source) => (
        <GrantRow
          key={source.id}
          client={client}
          source={source}
          neverWrite={neverWrite.includes(source.id)}
        />
      ))}
    </div>
  );
}

function GrantRow({ client, source, neverWrite }: { client: McpClient; source: DataSource; neverWrite: boolean }) {
  const current = grantLevel(client.grants, source.id);
  const unavailable = unavailableReason(source);
  const context = { revoked: !!client.revokedAt, unavailable, neverWrite };
  const note = unavailable ?? (neverWrite && current === "write" ? "Never write is on: its writes are refused." : null);

  return (
    <div
      role="row"
      className="grid grid-cols-[minmax(0,1fr)_auto_64px] items-center gap-3 border-b border-border px-3 py-1.5 last:border-b-0"
    >
      <div role="cell" className="min-w-0">
        <div className="flex min-w-0 items-center gap-1.5 text-[12.5px]">
          <span
            className="h-3 w-1 shrink-0 rounded-full"
            style={{ background: source.color ?? "transparent" }}
            aria-hidden
          />
          <EngineIcon engine={source.params.engine} />
          <span className={cx("truncate", unavailable ? "text-muted" : "text-fg")}>{source.name}</span>
          <span className="shrink-0 text-[11px] text-subtle">{ENGINE_LABEL[source.params.engine]}</span>
        </div>
        {note && (
          <div className="mt-0.5 flex items-baseline gap-1.5 pl-[22px] text-[11px] text-subtle">
            <span className="truncate" title={note}>
              {note}
            </span>
            {unavailable && (
              <button
                type="button"
                onClick={() => editDataSource(source.id)}
                className="shrink-0 text-accent hover:underline"
              >
                Edit…
              </button>
            )}
          </div>
        )}
      </div>
      <div role="cell">
        <Segmented
          label={`Access to ${source.name}`}
          value={current}
          onChange={(level) => void setAccess(client.id, source.id, level)}
          options={(["none", "read", "write"] as const).map((level) => ({
            value: level,
            label: LEVEL_LABEL[level],
            title: LEVEL_TITLE[level],
            blocked: levelBlocked(level, current, context),
            selectedClassName: level === "write" ? "text-warning" : level === "none" ? "text-muted" : undefined,
          }))}
        />
      </div>
      <div role="cell" className="flex justify-center">
        <button
          type="button"
          aria-pressed={neverWrite}
          aria-label={`Never write on ${source.name}`}
          title={neverWrite ? `${NEVER_WRITE_HINT} On.` : NEVER_WRITE_HINT}
          onClick={() => void setNeverWrite(source.id, !neverWrite)}
          className={cx(
            "inline-flex size-6 items-center justify-center rounded-md transition-colors hover:bg-hover",
            neverWrite ? "text-warning" : "text-subtle hover:text-fg",
          )}
        >
          {neverWrite ? <Lock className="size-3.5" /> : <LockOpen className="size-3.5" />}
        </button>
      </div>
    </div>
  );
}
