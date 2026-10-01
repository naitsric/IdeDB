import type { ApprovalRequest } from "./api";

/**
 * Writes waiting for the user, as the approval dialog walks them. Pure, so
 * the store and the tests share it.
 *
 * Requests arrive as events and from `mcp_approvals_pending` (after a
 * reload), in either order: one resolved while the list was loading must
 * not come back from that list. Ids grow for as long as the app runs, so
 * remembering the last resolved ones is enough.
 */
export interface ApprovalQueue {
  /** Oldest first. */
  requests: ApprovalRequest[];
  /** The request the user is looking at; the oldest when unset or gone. */
  selectedId: number | null;
  /** Recently resolved ids, newest last. */
  resolved: number[];
}

/** Resolved ids remembered; far more than can be in flight at once. */
const RESOLVED_KEPT = 200;

export const emptyQueue: ApprovalQueue = { requests: [], selectedId: null, resolved: [] };

export function addRequest(queue: ApprovalQueue, request: ApprovalRequest): ApprovalQueue {
  if (queue.resolved.includes(request.id) || queue.requests.some((r) => r.id === request.id)) return queue;
  const requests = [...queue.requests, request].sort((a, b) => a.id - b.id);
  return { ...queue, requests };
}

/** Adds what the server says is pending, e.g. after the UI reloaded. */
export function addPending(queue: ApprovalQueue, pending: readonly ApprovalRequest[]): ApprovalQueue {
  return pending.reduce(addRequest, queue);
}

/**
 * The request stopped waiting: answered (here or not), timed out or
 * withdrawn. When it was the one on screen, the next one takes its place.
 */
export function resolveRequest(queue: ApprovalQueue, id: number): ApprovalQueue {
  const resolved = queue.resolved.includes(id) ? queue.resolved : [...queue.resolved, id].slice(-RESOLVED_KEPT);
  const index = queue.requests.findIndex((r) => r.id === id);
  const requests = queue.requests.filter((r) => r.id !== id);
  const selectedId =
    queue.selectedId === id ? (requests[Math.min(index, requests.length - 1)]?.id ?? null) : queue.selectedId;
  return { requests, resolved, selectedId };
}

/** The request the dialog shows, and where it is in the queue. */
export function currentRequest(queue: ApprovalQueue): { request: ApprovalRequest; index: number } | null {
  if (queue.requests.length === 0) return null;
  const index = Math.max(
    0,
    queue.requests.findIndex((r) => r.id === queue.selectedId),
  );
  return { request: queue.requests[index], index };
}

/** Moves to the previous (-1) or next (+1) request, stopping at either end. */
export function stepRequest(queue: ApprovalQueue, step: -1 | 1): ApprovalQueue {
  const current = currentRequest(queue);
  if (!current) return queue;
  const next = queue.requests[Math.min(queue.requests.length - 1, Math.max(0, current.index + step))];
  return { ...queue, selectedId: next.id };
}

/**
 * Approve stays disabled this long after a request is shown, and after the
 * window comes back to the front, so a click or key meant for something
 * else cannot approve it.
 */
export const APPROVE_DELAY_MS = 800;

/** When the request on screen was shown, for {@link APPROVE_DELAY_MS}. */
export interface ApproveGate {
  requestId: number | null;
  shownAt: number;
}

export const closedGate: ApproveGate = { requestId: null, shownAt: 0 };

/** The gate for the request now on screen: re-armed when it is another one. */
export function showRequest(gate: ApproveGate, requestId: number | null, now: number): ApproveGate {
  return gate.requestId === requestId ? gate : { requestId, shownAt: now };
}

/** Re-arms the gate for the same request, e.g. when the window regains focus. */
export function rearm(gate: ApproveGate, now: number): ApproveGate {
  return { ...gate, shownAt: now };
}

/** Whether the request on screen has been there long enough to approve. */
export function armed(gate: ApproveGate, now: number): boolean {
  return gate.requestId !== null && now - gate.shownAt >= APPROVE_DELAY_MS;
}

/** How long until {@link armed} turns true; 0 once it is. */
export function armsIn(gate: ApproveGate, now: number): number {
  return gate.requestId === null ? 0 : Math.max(0, APPROVE_DELAY_MS - (now - gate.shownAt));
}
