import { create } from "zustand";
import type { Declaration } from "../editor/navigation";

export type RevealRequest = Declaration & { sourceId: string };

/**
 * A pending "show this in the Database Explorer" (⌘B from the editor). The
 * tree fulfils it once it is mounted and whatever the path needs (connection,
 * schema introspection, table columns) is loaded, then clears it.
 */
export const useExplorerReveal = create<{ request: RevealRequest | null }>(() => ({ request: null }));

export function revealInExplorer(request: RevealRequest) {
  useExplorerReveal.setState({ request });
}
