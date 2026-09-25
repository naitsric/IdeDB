import { create } from "zustand";

/**
 * A counter per data source that moves whenever its database may have
 * changed (a console ran a statement, the explorer refreshed), so cached
 * live diagnostics are rechecked: `select * from t` stops being an error
 * once `create table t` has run.
 */
export const useCheckRevisions = create<{ revisions: Record<string, number> }>(() => ({ revisions: {} }));

export function invalidateChecks(dataSourceId: string) {
  useCheckRevisions.setState((s) => ({
    revisions: { ...s.revisions, [dataSourceId]: (s.revisions[dataSourceId] ?? 0) + 1 },
  }));
}
