import { CircleAlert, Power, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Button, cx } from "../ui/primitives";
import { serverIsOff } from "./status";
import { loadMcp, setMcpTab, toggleServer, useMcp } from "./store";

/** Pieces the MCP tool window's tabs share. */

/** Says the initial load failed, with a retry. */
export function LoadErrorBanner() {
  const loadError = useMcp((s) => s.loadError);
  if (!loadError) return null;
  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-border bg-danger/10 px-3 py-1.5 text-[12px] text-danger">
      <CircleAlert className="size-3.5 shrink-0" />
      <span className="selectable min-w-0 flex-1 truncate" title={loadError}>
        Couldn't load the MCP server's state: {loadError}
      </span>
      <Button className="h-6" onClick={() => void loadMcp()}>
        Retry
      </Button>
    </div>
  );
}

/** Says when clients can't connect: the server is off or failed to start. */
export function ServerBanner() {
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

/** Whether the server is off on purpose (not starting, not failed): the banner's first case. */
export function useServerOff(): boolean {
  return useMcp((s) => s.loaded && serverIsOff(s.status, s.settings?.enabled ?? false));
}

export function EmptyState({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 overflow-y-auto p-6 text-center">
      <Icon className="size-6 text-subtle" strokeWidth={1.5} />
      <p className="text-[13px] font-medium text-fg">{title}</p>
      <div className="flex max-w-[420px] flex-col items-center text-[12px] text-muted">{children}</div>
    </div>
  );
}
