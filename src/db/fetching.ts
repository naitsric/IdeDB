import { create } from "zustand";

/**
 * Results are read a page at a time, as DataGrip does: a statement fetches
 * its first page and leaves the rest open on its session until the grid
 * scrolls near the end, the user fetches all, or the session moves on.
 */

/** Whether rows past the loaded ones exist, and whether they can still be fetched. */
export type MoreRows =
  /** Every row is loaded. */
  | "none"
  /** The rest is open on the session: scrolling or Fetch All loads it. */
  | "open"
  /** The rest was released (another statement, a submit, an idle timeout, a cancel): re-run to load it. */
  | "closed";

export const DEFAULT_PAGE_SIZE = 500;
export const PAGE_SIZE_CHOICES = [100, 500, 1000, 5000] as const;
const MAX_PAGE_SIZE = 1_000_000;
/** Scrolling within this many rows of the last loaded one fetches the next page. */
export const NEAR_END_ROWS = 100;

const STORAGE_KEY = "idedb.pageSize";

/** A page size typed by the user, or `null` when it is not a whole number in range. */
export function parsePageSize(text: string): number | null {
  if (!/^\s*\d+\s*$/.test(text)) return null;
  const n = Number.parseInt(text, 10);
  return n >= 1 && n <= MAX_PAGE_SIZE ? n : null;
}

function readPageSize(): number {
  try {
    return parsePageSize(localStorage.getItem(STORAGE_KEY) ?? "") ?? DEFAULT_PAGE_SIZE;
  } catch {
    return DEFAULT_PAGE_SIZE;
  }
}

export const useFetchSettings = create<{ pageSize: number; dialogOpen: boolean }>(() => ({
  pageSize: readPageSize(),
  dialogOpen: false,
}));

export function setPageSize(pageSize: number) {
  useFetchSettings.setState({ pageSize });
  try {
    localStorage.setItem(STORAGE_KEY, String(pageSize));
  } catch {
    // Not persisted; applies until the app quits.
  }
}

const count = new Intl.NumberFormat("en-US");

/** `500+ rows` while more can or could be loaded, `42 rows` when that is all. */
export function rowsLabel(loaded: number, more: MoreRows): string {
  if (more !== "none") return `${count.format(loaded)}+ rows`;
  return `${count.format(loaded)} ${loaded === 1 ? "row" : "rows"}`;
}

/** Whether scrolling to `lastVisibleRow` should fetch the next page. */
export function shouldFetchMore(lastVisibleRow: number, loaded: number, more: MoreRows, busy: boolean): boolean {
  return more === "open" && !busy && lastVisibleRow >= loaded - NEAR_END_ROWS;
}

/** What is left after a read finished: more rows open, none, or a rest a cancel released. */
export function moreAfter(done: { hasMore: boolean; cancelled: boolean }): MoreRows {
  if (done.hasMore) return "open";
  return done.cancelled ? "closed" : "none";
}
