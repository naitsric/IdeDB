import { api } from "../db/api";
import { useCheckRevisions } from "../db/checkRevisions";
import { effectiveSchema, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import type { LintBackend } from "./diagnostics";

/**
 * Where a console's live checks run: its data source's explorer session
 * (never the console's own, which may be busy with a long query), resolving
 * names in the console's current schema.
 */
function target(consoleId: string) {
  const entry = useConsoles.getState().consoles[consoleId];
  const explorer = entry && useDataSources.getState().explorers[entry.dataSourceId];
  if (!entry || explorer?.status !== "connected" || explorer.sessionId === undefined) return null;
  return { entry, sessionId: explorer.sessionId, schema: effectiveSchema(entry) };
}

function contextOf(consoleId: string): string | null {
  const t = target(consoleId);
  if (!t) return null;
  const revision = useCheckRevisions.getState().revisions[t.entry.dataSourceId] ?? 0;
  return `${t.sessionId}|${t.schema ?? ""}|${revision}`;
}

export function lintBackendFor(consoleId: string): LintBackend {
  return {
    context: () => contextOf(consoleId),
    async check(sql) {
      const t = target(consoleId);
      return t ? api.check(t.sessionId, sql, t.schema) : null;
    },
  };
}

/** A console's check context, re-rendering when it changes, so the editor rechecks. */
export function useLintContext(consoleId: string): string | null {
  // Subscribe to everything the context reads, then compute it from the latest state.
  const dataSourceId = useConsoles((s) => s.consoles[consoleId]?.dataSourceId);
  useConsoles((s) => s.consoles[consoleId]?.schema);
  useDataSources((s) => {
    const explorer = dataSourceId ? s.explorers[dataSourceId] : undefined;
    return `${explorer?.status}|${explorer?.sessionId}|${explorer?.server?.defaultSchema}`;
  });
  useCheckRevisions((s) => (dataSourceId ? s.revisions[dataSourceId] : undefined));
  return contextOf(consoleId);
}
