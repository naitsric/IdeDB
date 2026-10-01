import { ShieldAlert } from "lucide-react";
import { cx, StatusDot } from "../ui/primitives";
import { useNow } from "../ui/useNow";
import { toggleMcp } from "../workbench/Workbench";
import { onlineCount } from "./presence";
import { approvalsBadge, statusBarItem } from "./status";
import { showApprovals, useMcp } from "./store";

/**
 * The status bar's MCP item: whether the server runs, on which port, how
 * many clients are active, and a badge for writes waiting for approval.
 * Clicking it shows or hides the MCP tool window; the badge brings the
 * approval dialog back.
 */
export function McpStatusItem() {
  const status = useMcp((s) => s.status);
  const enabled = useMcp((s) => s.settings?.enabled ?? false);
  const clients = useMcp((s) => s.clients);
  const waiting = useMcp((s) => s.approvals.requests.length);
  const now = useNow(15_000);
  const { tone, text, title } = statusBarItem(status, enabled, onlineCount(clients, now));

  return (
    <span className="flex items-center gap-1">
      {waiting > 0 && (
        <button
          type="button"
          onClick={showApprovals}
          title="Writes waiting for your approval"
          className="flex items-center gap-1 rounded-full bg-warning/15 px-1.5 font-medium text-warning hover:bg-warning/25"
        >
          <ShieldAlert className="size-3" />
          {approvalsBadge(waiting)}
        </button>
      )}
      <button
        type="button"
        onClick={toggleMcp}
        title={title}
        className={cx(
          "flex items-center gap-1.5 rounded px-1 hover:bg-hover hover:text-fg",
          tone === "danger" && "text-danger hover:text-danger",
        )}
      >
        <StatusDot tone={tone} />
        <span className="tabular-nums">{text}</span>
      </button>
    </span>
  );
}
