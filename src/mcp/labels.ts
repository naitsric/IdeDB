import type { Decision, SqlWarning, Transport, WriteKind } from "./api";

/** What a write does, under its statement type in the approval dialog. */
export const WRITE_KIND_LABEL: Record<WriteKind, string> = {
  dml: "Changes data",
  ddl: "Changes the schema",
  privileges: "Changes users or privileges",
  procedural: "Runs procedural code",
  sideEffectFunction: "Calls a function that changes something",
  lockingRead: "Reads and locks rows",
  other: "Not a plain read",
  unparsed: "IdeDB couldn't analyze this statement",
};

/** Why a write deserves a second look. */
export const WARNING_LABEL: Record<SqlWarning, string> = {
  noWhereClause: "No WHERE clause: every row of the table is affected.",
  dropsOrTruncates: "Drops or truncates: the data is destroyed.",
};

/** A decision, as the Activity log's badge says it. */
export const DECISION_LABEL: Record<Decision, string> = {
  allowed: "Allowed",
  approved: "Approved",
  rejected: "Rejected",
  denied: "Denied",
  timeout: "Timed out",
  withdrawn: "Withdrawn",
};

/**
 * How a decision reads at a glance: calls that just ran stay quiet, writes
 * the user approved are green, refusals red, and requests nobody answered
 * amber.
 */
export const DECISION_TONE: Record<Decision, "neutral" | "success" | "danger" | "warning"> = {
  allowed: "neutral",
  approved: "success",
  rejected: "danger",
  denied: "danger",
  timeout: "warning",
  withdrawn: "warning",
};

/** What a decision meant for the call, in the Activity detail. */
export const DECISION_HINT: Record<Decision, string> = {
  allowed: "Ran without asking: reads need no approval.",
  approved: "Approved in IdeDB, then ran.",
  rejected: "Rejected in IdeDB: it didn't run.",
  denied: "Refused without asking: no access, never write, or a statement IdeDB doesn't run.",
  timeout: "Nobody answered in time: it didn't run.",
  withdrawn: "Withdrawn while it waited for approval: the client gave up, its token changed or the server stopped.",
};

/** How a client reached the server. */
export const TRANSPORT_LABEL: Record<Transport, string> = {
  http: "HTTP",
  bridge: "stdio bridge",
};
