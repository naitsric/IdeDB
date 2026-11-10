import { t, type MessageKey } from "../i18n";
import type { Decision, SqlWarning, Transport, WriteKind } from "./api";

/**
 * A record of texts read in the active language each time they are used, so
 * the lookup tables below keep their shape and follow a language change.
 */
function localized<K extends string>(keys: Record<K, MessageKey>): Record<K, string> {
  const texts = {} as Record<K, string>;
  for (const [id, key] of Object.entries(keys) as [K, MessageKey][]) {
    Object.defineProperty(texts, id, { enumerable: true, get: () => t(key) });
  }
  return texts;
}

/** What a write does, under its statement type in the approval dialog. */
export const WRITE_KIND_LABEL: Record<WriteKind, string> = localized({
  dml: "mcp.writeKind.dml",
  ddl: "mcp.writeKind.ddl",
  privileges: "mcp.writeKind.privileges",
  procedural: "mcp.writeKind.procedural",
  sideEffectFunction: "mcp.writeKind.sideEffectFunction",
  lockingRead: "mcp.writeKind.lockingRead",
  other: "mcp.writeKind.other",
  unparsed: "mcp.writeKind.unparsed",
});

/** Why a write deserves a second look. */
export const WARNING_LABEL: Record<SqlWarning, string> = localized({
  noWhereClause: "mcp.warning.noWhereClause",
  dropsOrTruncates: "mcp.warning.dropsOrTruncates",
});

/** A decision, as the Activity log's badge says it. */
export const DECISION_LABEL: Record<Decision, string> = localized({
  allowed: "mcp.decision.allowed",
  approved: "mcp.decision.approved",
  rejected: "mcp.decision.rejected",
  denied: "mcp.decision.denied",
  timeout: "mcp.decision.timeout",
  withdrawn: "mcp.decision.withdrawn",
});

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
export const DECISION_HINT: Record<Decision, string> = localized({
  allowed: "mcp.decisionHint.allowed",
  approved: "mcp.decisionHint.approved",
  rejected: "mcp.decisionHint.rejected",
  denied: "mcp.decisionHint.denied",
  timeout: "mcp.decisionHint.timeout",
  withdrawn: "mcp.decisionHint.withdrawn",
});

/** How a client reached the server. */
export const TRANSPORT_LABEL: Record<Transport, string> = localized({
  http: "mcp.transport.http",
  bridge: "mcp.transport.bridge",
});
