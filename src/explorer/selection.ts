import { create } from "zustand";

/** What is selected in the Database Explorer; commands act on it. */
export type ExplorerSelection =
  | { kind: "dataSource"; sourceId: string }
  | { kind: "schema"; sourceId: string; schema: string }
  | { kind: "table"; sourceId: string; schema: string; table: string }
  | { kind: "column"; sourceId: string; schema: string; table: string; column: string };

export const useExplorerSelection = create<{ selection: ExplorerSelection | null }>(() => ({ selection: null }));
