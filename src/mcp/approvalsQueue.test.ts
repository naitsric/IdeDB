import { describe, expect, it } from "vitest";
import type { ApprovalRequest } from "./api";
import {
  addPending,
  addRequest,
  APPROVE_DELAY_MS,
  armed,
  armsIn,
  closedGate,
  currentRequest,
  emptyQueue,
  rearm,
  resolveRequest,
  showRequest,
  stepRequest,
  type ApprovalQueue,
} from "./approvalsQueue";

const request = (id: number, extra: Partial<ApprovalRequest> = {}): ApprovalRequest => ({
  id,
  clientId: "c1",
  clientName: "claude-code",
  clientInfo: null,
  dataSourceId: "ds",
  dataSourceName: "shop",
  dataSourceColor: null,
  sql: `delete from t where id = ${id}`,
  summary: "DELETE",
  writeKind: "dml",
  warnings: [],
  reason: null,
  requestedAt: "2026-10-01T10:00:00.000Z",
  expiresAt: "2026-10-01T10:02:00.000Z",
  ...extra,
});

const ids = (queue: ApprovalQueue) => queue.requests.map((r) => r.id);
const current = (queue: ApprovalQueue) => currentRequest(queue)?.request.id;

describe("approvals queue", () => {
  it("keeps requests oldest first, once each", () => {
    let queue = addRequest(emptyQueue, request(3));
    queue = addRequest(queue, request(1));
    queue = addRequest(queue, request(2));
    queue = addRequest(queue, request(1));
    expect(ids(queue)).toEqual([1, 2, 3]);
  });

  it("shows the oldest unless another one is selected", () => {
    expect(currentRequest(emptyQueue)).toBeNull();
    const queue = addPending(emptyQueue, [request(5), request(7)]);
    expect(currentRequest(queue)).toEqual({ request: queue.requests[0], index: 0 });
    // A newer request does not take the screen from the one being read.
    expect(current(addRequest(queue, request(9)))).toBe(5);
    expect(current(addRequest(stepRequest(queue, 1), request(9)))).toBe(7);
    expect(currentRequest(addRequest(stepRequest(queue, 1), request(9)))?.index).toBe(1);
  });

  it("steps between requests and stops at either end", () => {
    let queue = addPending(emptyQueue, [request(1), request(2), request(3)]);
    queue = stepRequest(queue, -1);
    expect(current(queue)).toBe(1);
    queue = stepRequest(stepRequest(stepRequest(queue, 1), 1), 1);
    expect(currentRequest(queue)).toMatchObject({ index: 2, request: { id: 3 } });
    expect(stepRequest(emptyQueue, 1)).toBe(emptyQueue);
  });

  it("drops a resolved request and shows the next one in its place", () => {
    let queue = stepRequest(addPending(emptyQueue, [request(1), request(2), request(3)]), 1);
    expect(current(queue)).toBe(2);
    queue = resolveRequest(queue, 2);
    expect(ids(queue)).toEqual([1, 3]);
    expect(current(queue)).toBe(3);
    // The last one resolved: the one before it is shown.
    queue = resolveRequest(queue, 3);
    expect(current(queue)).toBe(1);
    queue = resolveRequest(queue, 1);
    expect(currentRequest(queue)).toBeNull();
  });

  it("keeps the selection when another request resolves", () => {
    let queue = stepRequest(addPending(emptyQueue, [request(1), request(2), request(3)]), 1);
    queue = resolveRequest(queue, 1);
    expect(current(queue)).toBe(2);
    expect(currentRequest(queue)?.index).toBe(0);
  });

  it("never brings back a request resolved while the pending list loaded", () => {
    // The event arrived first; the list was read before the request resolved.
    let queue = resolveRequest(emptyQueue, 8);
    queue = addPending(queue, [request(8), request(9)]);
    expect(ids(queue)).toEqual([9]);
    expect(addRequest(queue, request(8))).toBe(queue);
  });

  it("remembers a bounded number of resolved ids", () => {
    let queue = emptyQueue;
    for (let id = 1; id <= 500; id++) queue = resolveRequest(queue, id);
    expect(queue.resolved.length).toBe(200);
    expect(queue.resolved.at(-1)).toBe(500);
    expect(resolveRequest(queue, 500).resolved).toBe(queue.resolved);
  });
});

describe("approve gate", () => {
  it("arms 800 ms after a request is shown", () => {
    const gate = showRequest(closedGate, 1, 10_000);
    expect(armed(gate, 10_000)).toBe(false);
    expect(armed(gate, 10_000 + APPROVE_DELAY_MS - 1)).toBe(false);
    expect(armed(gate, 10_000 + APPROVE_DELAY_MS)).toBe(true);
    expect(armsIn(gate, 10_300)).toBe(APPROVE_DELAY_MS - 300);
    expect(armsIn(gate, 20_000)).toBe(0);
  });

  it("re-arms when another request is shown, not when the same one renders again", () => {
    const first = showRequest(closedGate, 1, 0);
    expect(showRequest(first, 1, 5_000)).toBe(first);
    const second = showRequest(first, 2, 5_000);
    expect(armed(second, 5_000 + APPROVE_DELAY_MS - 1)).toBe(false);
    expect(armed(second, 5_000 + APPROVE_DELAY_MS)).toBe(true);
  });

  it("re-arms when the window comes back to the front", () => {
    const gate = rearm(showRequest(closedGate, 1, 0), 60_000);
    expect(armed(gate, 60_000 + 100)).toBe(false);
    expect(armed(gate, 60_000 + APPROVE_DELAY_MS)).toBe(true);
  });

  it("is never armed without a request", () => {
    expect(armed(closedGate, 1e12)).toBe(false);
    expect(armed(showRequest(showRequest(closedGate, 1, 0), null, 0), 1e12)).toBe(false);
    expect(armsIn(closedGate, 0)).toBe(0);
  });
});
