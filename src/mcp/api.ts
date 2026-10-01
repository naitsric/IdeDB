import { invoke } from "@tauri-apps/api/core";

/*
 * Mirrors of the Rust types of the MCP server: crates/idedb-mcp,
 * crates/idedb-store/src/mcp.rs, crates/idedb-sql and src-tauri/src/mcp.rs.
 */

export interface McpSettings {
  /** Whether the server listens. Off until the user turns it on. */
  enabled: boolean;
  /** The loopback port it listens on. */
  port: number;
  /** Rows a read returns unless the client asks for another number (up to 1000). */
  maxRows: number;
  /** Reads still running after this long are cancelled. */
  statementTimeoutSecs: number;
  /** Approved writes still running after this long are cancelled. */
  writeTimeoutSecs: number;
  /** A write nobody approves or rejects within this long is refused. */
  approvalTimeoutSecs: number;
}

export interface ServerStatus {
  running: boolean;
  /** The port it listens on, while running. */
  port: number | null;
  /** The MCP endpoint while running, e.g. `http://127.0.0.1:7412/mcp`. */
  url: string | null;
  /** Why it is not running, when starting it failed. */
  error: string | null;
}

/** What a client may do on a data source. Writes also need the user's approval each time. */
export type Access = "read" | "write";

export interface Grant {
  dataSourceId: string;
  access: Access;
}

/** A registered MCP client. Its token is shown once, when created or regenerated. */
export interface McpClient {
  id: string;
  name: string;
  /** The token's first characters, to tell tokens apart. */
  tokenPrefix: string;
  /** UTC, RFC 3339, like every timestamp here. */
  createdAt: string;
  /** Its last authenticated request. */
  lastSeenAt: string | null;
  /** The `clientInfo` it last declared; unverified. */
  lastClientName: string | null;
  lastClientVersion: string | null;
  /** Set once revoked: its token no longer works. */
  revokedAt: string | null;
  grants: Grant[];
}

/** How a tool call was let through or stopped. */
export type Decision = "allowed" | "approved" | "rejected" | "denied" | "timeout" | "withdrawn";

/** How the client reached the server: directly, or through the stdio bridge. */
export type Transport = "http" | "bridge";

/** One tool call in the audit log. */
export interface AuditEntry {
  id: number;
  at: string;
  clientId: string | null;
  /** As registered in IdeDB when the call ran. */
  clientName: string;
  /** What the client declared itself to be; unverified. */
  clientInfoName: string | null;
  clientInfoVersion: string | null;
  protocolVersion: string | null;
  transport: Transport;
  sessionKey: string | null;
  tool: string;
  dataSourceId: string | null;
  dataSourceName: string | null;
  sql: string | null;
  /** The SQL was longer and is cut to its first 100 KiB. */
  sqlTruncated: boolean;
  statementKind: string | null;
  reason: string | null;
  decision: Decision;
  approvalWaitMs: number | null;
  elapsedMs: number | null;
  /** Rows returned, or affected for statements without a result set. */
  rowCount: number | null;
  /** The result sent back was cut short. */
  truncated: boolean;
  error: string | null;
}

/** Audit rows matching every condition given, newest first. */
export interface AuditFilter {
  clientId?: string | null;
  dataSourceId?: string | null;
  decision?: Decision | null;
  /** Contained in the SQL, case-insensitive. */
  search?: string | null;
  /** Only rows older than this one: the last id of the previous page. */
  beforeId?: number | null;
  limit?: number;
}

/** What kind of write a statement is, as idedb-sql classifies it. */
export type WriteKind =
  | "dml"
  | "ddl"
  | "privileges"
  | "procedural"
  | "sideEffectFunction"
  | "lockingRead"
  | "other"
  | "unparsed";

/** Something the person approving a write should look at twice. */
export type SqlWarning = "noWhereClause" | "dropsOrTruncates";

/** An MCP client's self-reported `clientInfo`. */
export interface ClientInfo {
  name: string;
  version: string | null;
}

/** A write waiting for the user. */
export interface ApprovalRequest {
  id: number;
  clientId: string;
  /** As registered in IdeDB: the verified identity. */
  clientName: string;
  /** What the client declared itself to be; unverified. */
  clientInfo: ClientInfo | null;
  dataSourceId: string;
  dataSourceName: string;
  dataSourceColor: string | null;
  sql: string;
  /** The statement type, e.g. `DELETE` or `CREATE TABLE`. */
  summary: string;
  writeKind: WriteKind;
  warnings: SqlWarning[];
  /** Why the client says it runs the statement. */
  reason: string | null;
  requestedAt: string;
  /** When it is refused unless answered. */
  expiresAt: string;
}

/** What the server reports on `mcp://event`. */
export type McpEvent =
  | ({ kind: "status" } & ServerStatus)
  | ({ kind: "audit" } & AuditEntry)
  | ({ kind: "approvalRequested" } & ApprovalRequest)
  /** Answered, timed out or withdrawn. */
  | { kind: "approvalResolved"; id: number; decision: Decision }
  /** A client authenticated; `at` is its new `lastSeenAt`. Sent at most every 30 s per client. */
  | { kind: "clientSeen"; clientId: string; at: string };

export const MCP_EVENT = "mcp://event";

/** A client and its token, which is never shown again. */
export interface ClientWithToken {
  client: McpClient;
  token: string;
}

/** Where clients reach the server. */
export interface Endpoint {
  /** The port it listens on, else the saved one, e.g. `http://127.0.0.1:7412/mcp`. */
  url: string;
  /**
   * The executable stdio-only clients (Claude Desktop) run as `<it> mcp-bridge`: the one the app runs
   * from, so a dev build names the dev binary. Null when the app can't tell.
   */
  bridgeCommand: string | null;
}

export const mcpApi = {
  status: () => invoke<ServerStatus>("mcp_status"),

  settings: () => invoke<McpSettings>("mcp_settings_get"),

  /** Saves, then starts, restarts or stops the server to match. A failed start is in the status. */
  saveSettings: (settings: McpSettings) => invoke<ServerStatus>("mcp_settings_save", { settings }),

  /** Revoked ones included, oldest first. */
  clients: () => invoke<McpClient[]>("mcp_clients_list"),

  createClient: (name: string) => invoke<ClientWithToken>("mcp_client_create", { name }),

  renameClient: (id: string, name: string) => invoke<McpClient>("mcp_client_rename", { id, name }),

  /** A new token; the old one stops working and its pending approvals are withdrawn. */
  rotateClient: (id: string) => invoke<ClientWithToken>("mcp_client_rotate", { id }),

  revokeClient: (id: string) => invoke<McpClient>("mcp_client_revoke", { id }),

  deleteClient: (id: string) => invoke<void>("mcp_client_delete", { id }),

  /** Replaces all of the client's grants. */
  setGrants: (clientId: string, grants: Grant[]) => invoke<McpClient>("mcp_grants_set", { clientId, grants }),

  /** Ids of the data sources marked never-write. */
  neverWrite: () => invoke<string[]>("mcp_never_write_list"),

  setNeverWrite: (dataSourceId: string, value: boolean) =>
    invoke<void>("mcp_never_write_set", { dataSourceId, value }),

  audit: (filter: AuditFilter) => invoke<AuditEntry[]>("mcp_audit_list", { filter: { limit: 100, ...filter } }),

  /** False when it was no longer pending. */
  answerApproval: (id: number, approve: boolean) => invoke<boolean>("mcp_approval_answer", { id, approve }),

  /** Oldest first. */
  pendingApprovals: () => invoke<ApprovalRequest[]>("mcp_approvals_pending"),

  endpoint: () => invoke<Endpoint>("mcp_endpoint"),
};
