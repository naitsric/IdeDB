import { describe, expect, it } from "vitest";
import type { ApprovalRequest, AuditEntry, McpClient, McpEvent } from "./api";
import { currentRequest, emptyQueue } from "./approvalsQueue";
import { applyEvent, AUDIT_RING, mergeAudit, type McpSnapshot } from "./events";

const client = (id: string): McpClient => ({
  id,
  name: id,
  tokenPrefix: "idedb_abcdef",
  createdAt: "2026-09-01T12:00:00.000Z",
  lastSeenAt: null,
  lastClientName: null,
  lastClientVersion: null,
  revokedAt: null,
  grants: [],
});

const audit = (id: number, extra: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  at: "2026-10-01T12:00:00.000Z",
  clientId: "c1",
  clientName: "claude-code",
  clientInfoName: null,
  clientInfoVersion: null,
  protocolVersion: "2026-07-28",
  transport: "http",
  sessionKey: null,
  tool: "query",
  dataSourceId: "ds",
  dataSourceName: "shop",
  sql: "select 1",
  sqlTruncated: false,
  statementKind: "SELECT",
  reason: null,
  decision: "allowed",
  approvalWaitMs: null,
  elapsedMs: 3,
  rowCount: 1,
  truncated: false,
  error: null,
  ...extra,
});

const approval = (id: number): ApprovalRequest => ({
  id,
  clientId: "c1",
  clientName: "claude-code",
  clientInfo: { name: "claude-code", version: "2.1.0" },
  dataSourceId: "ds",
  dataSourceName: "shop",
  dataSourceColor: "#e5484d",
  sql: "delete from t",
  summary: "DELETE",
  writeKind: "dml",
  warnings: ["noWhereClause"],
  reason: "clean up",
  requestedAt: "2026-10-01T12:00:00.000Z",
  expiresAt: "2026-10-01T12:02:00.000Z",
});

const initial: McpSnapshot = {
  status: { running: false, port: null, url: null, error: null },
  clients: [client("c1"), client("c2")],
  approvals: emptyQueue,
  audit: [],
  approvalsHidden: false,
};

const apply = (state: McpSnapshot, event: McpEvent): McpSnapshot => ({ ...state, ...applyEvent(state, event) });

describe("applyEvent", () => {
  it("takes the server status without the event tag", () => {
    const next = apply(initial, { kind: "status", running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null });
    expect(next.status).toEqual({ running: true, port: 7412, url: "http://127.0.0.1:7412/mcp", error: null });
  });

  it("queues approvals, brings the dialog back, and drops them when resolved", () => {
    let state = apply({ ...initial, approvalsHidden: true }, { kind: "approvalRequested", ...approval(1) });
    expect(state.approvalsHidden).toBe(false);
    expect(currentRequest(state.approvals)?.request).toEqual(approval(1));
    expect(currentRequest(state.approvals)?.request).not.toHaveProperty("kind");

    state = apply(state, { kind: "approvalRequested", ...approval(2) });
    state = apply(state, { kind: "approvalResolved", id: 1, decision: "timeout" });
    expect(state.approvals.requests.map((r) => r.id)).toEqual([2]);
  });

  it("marks a client seen", () => {
    const next = apply(initial, { kind: "clientSeen", clientId: "c2", at: "2026-10-01T12:00:00.000Z" });
    expect(next.clients.map((c) => c.lastSeenAt)).toEqual([null, "2026-10-01T12:00:00.000Z"]);
  });

  it("records audit rows and the clientInfo they declare", () => {
    let state = apply(initial, { kind: "audit", ...audit(1) });
    expect(state.audit).toEqual([audit(1)]);
    expect(state.clients).toBe(initial.clients);

    state = apply(state, { kind: "audit", ...audit(2, { clientInfoName: "claude-code", clientInfoVersion: "2.1.0" }) });
    expect(state.audit.map((e) => e.id)).toEqual([2, 1]);
    expect(state.clients[0]).toMatchObject({ lastClientName: "claude-code", lastClientVersion: "2.1.0" });
    expect(state.clients[1].lastClientName).toBeNull();

    // The same clientInfo again changes nothing.
    const clients = state.clients;
    state = apply(state, { kind: "audit", ...audit(3, { clientInfoName: "claude-code", clientInfoVersion: "2.1.0" }) });
    expect(state.clients).toBe(clients);
  });
});

describe("mergeAudit", () => {
  it("keeps rows once each, newest first", () => {
    const merged = mergeAudit([audit(5), audit(3)], [audit(4), audit(5), audit(1)]);
    expect(merged.map((e) => e.id)).toEqual([5, 4, 3, 1]);
  });

  it("drops the oldest past the ring's size", () => {
    const loaded = Array.from({ length: AUDIT_RING }, (_, i) => audit(AUDIT_RING - i));
    const merged = mergeAudit(loaded, [audit(AUDIT_RING + 1)]);
    expect(merged).toHaveLength(AUDIT_RING);
    expect(merged[0].id).toBe(AUDIT_RING + 1);
    expect(merged.at(-1)?.id).toBe(2);
  });
});
