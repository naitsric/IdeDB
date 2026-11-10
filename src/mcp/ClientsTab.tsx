import { Activity, Ban, Bot, Lock, LockOpen, Pencil, Plus, RotateCw, Trash2 } from "lucide-react";
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
  openNewClient,
  openRename,
  regenerateToken,
  revokeClient,
  selectClient,
  setAccess,
  setNeverWrite,
  showClientActivity,
  useMcp,
} from "./store";
import { EmptyState, LoadErrorBanner, ServerBanner } from "./TabParts";
import { useT, type MessageKey } from "../i18n";

/** How often presence ("Online", "5 min ago") is worked out again. */
const PRESENCE_TICK_MS = 15_000;

/** The MCP clients, and what the selected one may do on each data source. */
export function ClientsTab() {
  const t = useT();
  const clients = useMcp((s) => s.clients);
  const loaded = useMcp((s) => s.loaded);
  const selectedId = useMcp((s) => s.selectedClientId);
  const now = useNow(PRESENCE_TICK_MS);
  const selected = clients.find((c) => c.id === selectedId) ?? clients[0];

  let body: ReactNode = null;
  if (loaded && clients.length === 0) {
    body = (
      <EmptyState icon={Bot} title={t("mcp.clients.none.title")}>
        {t("mcp.clients.none.body")}
        <Button variant="primary" className="mt-3" onClick={openNewClient}>
          <Plus className="size-3.5" /> {t("mcp.activity.newClient")}
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
      <LoadErrorBanner />
      <ServerBanner />
      {body}
    </div>
  );
}

function ClientList({ clients, selected, now }: { clients: McpClient[]; selected?: McpClient; now: number }) {
  const t = useT();
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
        <IconButton label={t("mcp.activity.newClient")} onClick={openNewClient}>
          <Plus className="size-4" />
        </IconButton>
        <span className="ml-auto px-1.5 text-[11px] text-subtle tabular-nums">
          {clients.length} {t(clients.length === 1 ? "mcp.clients.one" : "mcp.clients.many")}
        </span>
      </div>
      <div
        ref={list}
        role="listbox"
        aria-label={t("mcp.clients.listLabel")}
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
  const t = useT();
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
                {revoked ? t("mcp.clients.revoked") : presenceShort(client.lastSeenAt, now)}
              </span>
            </div>
            <div className="flex min-w-0 items-baseline gap-1.5 text-[11px] text-subtle">
              <span className="shrink-0 font-mono">{client.tokenPrefix}…</span>
              {info && (
                <span className="truncate" title={t("mcp.clients.reportedBy", { info })}>
                  · {info}
                </span>
              )}
            </div>
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem label={t("mcp.clients.showActivity")} onSelect={() => showClientActivity(client.id)} />
        <ContextMenuSeparator />
        <ContextMenuItem label={t("mcp.clients.rename")} onSelect={() => openRename(client)} />
        <ContextMenuItem label={t("mcp.clients.regenerate")} disabled={revoked} onSelect={() => void regenerateToken(client)} />
        <ContextMenuSeparator />
        <ContextMenuItem label={t("mcp.clients.revoke")} disabled={revoked} onSelect={() => void revokeClient(client)} />
        <ContextMenuItem label={t("mcp.clients.delete")} onSelect={() => void deleteClient(client)} />
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}

function ClientDetail({ client, now }: { client: McpClient; now: number }) {
  const t = useT();
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
              {revoked ? t("mcp.clients.revoked") : presenceLabel(client.lastSeenAt, now)}
            </span>
          </div>
          <p className="mt-0.5 flex flex-wrap gap-x-3 text-[11.5px] text-subtle">
            <span>
              {t("mcp.clients.token")} <span className="font-mono text-muted">{client.tokenPrefix}…</span>
            </span>
            <span>{t("mcp.clients.created", { date: formatDay(client.createdAt, now) })}</span>
            {info && (
              <span title={t("mcp.clients.reportedHint")}>
                {t("mcp.detail.reportedAs")} <span className="text-muted">{info}</span>
              </span>
            )}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-0.5">
          <IconButton label={t("mcp.clients.showActivity")} onClick={() => showClientActivity(client.id)}>
            <Activity className="size-3.5" />
          </IconButton>
          <IconButton label={t("mcp.clients.rename")} onClick={() => openRename(client)}>
            <Pencil className="size-3.5" />
          </IconButton>
          <IconButton label={t("mcp.clients.regenerate")} disabled={revoked} onClick={() => void regenerateToken(client)}>
            <RotateCw className="size-3.5" />
          </IconButton>
          <IconButton label={t("mcp.clients.revoke")} disabled={revoked} onClick={() => void revokeClient(client)}>
            <Ban className="size-3.5" />
          </IconButton>
          <IconButton label={t("mcp.clients.delete")} className="hover:text-danger" onClick={() => void deleteClient(client)}>
            <Trash2 className="size-3.5" />
          </IconButton>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 py-3">
        {revoked && (
          <Notice tone="danger" icon={Ban}>
            {t("mcp.clients.revokedNotice", { date: formatDay(client.revokedAt!, now) })}
          </Notice>
        )}
        <section className="flex flex-col gap-2">
          <SectionTitle
            title={t("mcp.clients.access")}
            hint={t("mcp.clients.accessHint")}
          />
          <GrantsMatrix client={client} />
        </section>
        {!revoked && port !== null && (
          <section className="flex flex-col gap-2">
            <SectionTitle
              title={t("mcp.clients.connect")}
              hint={t("mcp.clients.connectHint", { placeholder: TOKEN_PLACEHOLDER })}
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

const LEVEL_LABEL: Record<GrantLevel, MessageKey> = { none: "mcp.level.none", read: "mcp.level.read", write: "mcp.level.write" };
const LEVEL_TITLE: Record<GrantLevel, MessageKey> = {
  none: "mcp.level.noneHint",
  read: "mcp.level.readHint",
  write: "mcp.level.writeHint",
};

/** One row per data source: the client's access, and the never-write lock for every client. */
function GrantsMatrix({ client }: { client: McpClient }) {
  const t = useT();
  const sources = useDataSources((s) => s.sources);
  const loaded = useDataSources((s) => s.loaded);
  const neverWrite = useMcp((s) => s.neverWrite);

  if (loaded && sources.length === 0) {
    return (
      <div className="flex flex-col items-center gap-2 rounded-md border border-dashed border-border-strong px-4 py-5 text-center">
        <p className="text-[12px] text-muted">{t("mcp.clients.noSources")}</p>
        <Button onClick={() => executeCommand("datasource.new")}>
          <Plus className="size-3.5" /> {t("command.datasource.new")}
        </Button>
      </div>
    );
  }

  return (
    <div role="table" aria-label={t("mcp.clients.accessOf", { name: client.name })} className="rounded-md border border-border">
      <div
        role="row"
        className="grid h-7 grid-cols-[minmax(0,1fr)_auto_64px] items-center gap-3 border-b border-border px-3 text-[11px] text-subtle"
      >
        <span role="columnheader">{t("mcp.activity.dataSource")}</span>
        <span role="columnheader" className="w-[138px]">
          {t("mcp.clients.access")}
        </span>
        <span role="columnheader" className="text-center" title={t("mcp.clients.neverWriteHint")}>
          {t("mcp.clients.neverWrite")}
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
  const t = useT();
  const current = grantLevel(client.grants, source.id);
  const unavailable = unavailableReason(source);
  const context = { revoked: !!client.revokedAt, unavailable, neverWrite };
  const note = unavailable ?? (neverWrite && current === "write" ? t("mcp.clients.neverWriteNote") : null);

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
                {t("mcp.clients.edit")}
              </button>
            )}
          </div>
        )}
      </div>
      <div role="cell">
        <Segmented
          label={t("mcp.clients.accessTo", { name: source.name })}
          value={current}
          onChange={(level) => void setAccess(client.id, source.id, level)}
          options={(["none", "read", "write"] as const).map((level) => ({
            value: level,
            label: t(LEVEL_LABEL[level]),
            title: t(LEVEL_TITLE[level]),
            blocked: levelBlocked(level, current, context),
            selectedClassName: level === "write" ? "text-warning" : level === "none" ? "text-muted" : undefined,
          }))}
        />
      </div>
      <div role="cell" className="flex justify-center">
        <button
          type="button"
          aria-pressed={neverWrite}
          aria-label={t("mcp.clients.neverWriteOn", { name: source.name })}
          title={neverWrite ? `${t("mcp.clients.neverWriteHint")} ${t("mcp.clients.on")}` : t("mcp.clients.neverWriteHint")}
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
