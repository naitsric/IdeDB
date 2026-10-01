import type { AuditEntry, McpClient, McpEvent, ServerStatus } from "./api";
import { addRequest, resolveRequest, type ApprovalQueue } from "./approvalsQueue";

/** Audit rows kept in memory for the Activity view, newest first. */
export const AUDIT_RING = 500;

/** The part of the MCP store that server events change. */
export interface McpSnapshot {
  status: ServerStatus;
  clients: McpClient[];
  approvals: ApprovalQueue;
  /** Newest first, at most {@link AUDIT_RING}. */
  audit: AuditEntry[];
  /** The approval dialog was put aside (Esc) while requests wait. */
  approvalsHidden: boolean;
}

/** What an `mcp://event` changes. Pure, for the store and its tests. */
export function applyEvent(state: McpSnapshot, event: McpEvent): Partial<McpSnapshot> {
  switch (event.kind) {
    case "status": {
      const { kind: _, ...status } = event;
      return { status };
    }
    case "audit": {
      const { kind: _, ...entry } = event;
      return { audit: mergeAudit(state.audit, [entry]), clients: noteClientInfo(state.clients, entry) };
    }
    case "approvalRequested": {
      const { kind: _, ...request } = event;
      // A new request always comes to the front, even after Esc put the dialog aside.
      return { approvals: addRequest(state.approvals, request), approvalsHidden: false };
    }
    case "approvalResolved":
      return { approvals: resolveRequest(state.approvals, event.id) };
    case "clientSeen":
      return {
        clients: state.clients.map((c) => (c.id === event.clientId ? { ...c, lastSeenAt: event.at } : c)),
      };
  }
}

/** Adds audit rows to the ring: once each, newest first, the oldest dropped past its size. */
export function mergeAudit(ring: readonly AuditEntry[], entries: readonly AuditEntry[]): AuditEntry[] {
  const byId = new Map(ring.map((e) => [e.id, e]));
  for (const entry of entries) byId.set(entry.id, entry);
  return [...byId.values()].sort((a, b) => b.id - a.id).slice(0, AUDIT_RING);
}

/** A call records the `clientInfo` it declared, as the store does on the client. */
function noteClientInfo(clients: McpClient[], entry: AuditEntry): McpClient[] {
  if (!entry.clientId || !entry.clientInfoName) return clients;
  const client = clients.find((c) => c.id === entry.clientId);
  const same = client?.lastClientName === entry.clientInfoName && client.lastClientVersion === entry.clientInfoVersion;
  if (!client || same) return clients;
  return clients.map((c) =>
    c.id === entry.clientId
      ? { ...c, lastClientName: entry.clientInfoName, lastClientVersion: entry.clientInfoVersion }
      : c,
  );
}
