import { Activity, Bot, Server, type LucideIcon } from "lucide-react";
import { useEffect, useRef, type ComponentType, type KeyboardEvent } from "react";
import { cx, StatusDot } from "../ui/primitives";
import { ActivityTab } from "./ActivityTab";
import { ClientsTab } from "./ClientsTab";
import { ServerTab } from "./ServerTab";
import { serverState } from "./status";
import { reloadClients, setMcpTab, useMcp, type McpTab } from "./store";

const TABS: { id: McpTab; label: string; icon: LucideIcon }[] = [
  { id: "activity", label: "Activity", icon: Activity },
  { id: "clients", label: "Clients", icon: Bot },
  { id: "server", label: "Server", icon: Server },
];

const TAB_CONTENT: Record<McpTab, ComponentType> = {
  activity: ActivityTab,
  clients: ClientsTab,
  server: ServerTab,
};

/**
 * The MCP tool window: what clients do through IdeDB, who may reach the
 * user's data sources and with which access, and the server they reach
 * it on.
 */
export function McpPanel() {
  const tab = useMcp((s) => s.tab);
  const tabs = useRef<HTMLDivElement>(null);
  const Content = TAB_CONTENT[tab];

  // Presence moves without events in between (seen at most every 30 s).
  useEffect(() => {
    void reloadClients();
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const index = TABS.findIndex((t) => t.id === tab);
    const next = TABS[(index + (e.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length];
    setMcpTab(next.id);
    tabs.current?.querySelector<HTMLElement>(`[data-tab="${next.id}"]`)?.focus();
  };

  return (
    <div className="flex h-full flex-col bg-panel">
      <div className="flex h-8 shrink-0 items-stretch border-b border-border bg-bg">
        <div ref={tabs} role="tablist" aria-label="MCP" className="flex items-stretch" onKeyDown={onKeyDown}>
          {TABS.map(({ id, label, icon: Icon }) => {
            const active = id === tab;
            return (
              <button
                key={id}
                type="button"
                role="tab"
                data-tab={id}
                aria-selected={active}
                aria-controls={`mcp-${id}`}
                tabIndex={active ? 0 : -1}
                onClick={() => setMcpTab(id)}
                className={cx(
                  "flex items-center gap-1.5 border-r border-border px-3 text-[12px] transition-colors",
                  active ? "bg-panel text-fg shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:bg-hover hover:text-fg",
                )}
              >
                <Icon className="size-3.5" />
                {label}
              </button>
            );
          })}
        </div>
        <ServerSummary />
      </div>
      <div id={`mcp-${tab}`} role="tabpanel" className="flex min-h-0 flex-1 flex-col">
        <Content />
      </div>
    </div>
  );
}

/** Whether the server is on, at the right of the tabs; opens the Server tab. Its error is the tooltip. */
function ServerSummary() {
  const status = useMcp((s) => s.status);
  const enabled = useMcp((s) => s.settings?.enabled ?? false);
  const { tone, label } = serverState(status, enabled);
  return (
    <button
      type="button"
      onClick={() => setMcpTab("server")}
      title={status.error ?? undefined}
      className="ml-auto flex min-w-0 items-center gap-1.5 px-3 text-[11.5px] text-muted hover:text-fg"
    >
      <StatusDot tone={tone} />
      <span className={cx("truncate", tone === "danger" && "text-danger")}>{label}</span>
    </button>
  );
}
