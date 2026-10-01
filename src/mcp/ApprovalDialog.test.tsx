// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApprovalRequest } from "./api";

const answered: [number, boolean][] = [];

vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./api")>()),
  mcpApi: {
    answerApproval: vi.fn(async (id: number, approve: boolean) => {
      answered.push([id, approve]);
      return true;
    }),
  },
}));

// Imported after the mock.
const { ApprovalDialog } = await import("./ApprovalDialog");
const { addRequest, APPROVE_DELAY_MS, emptyQueue } = await import("./approvalsQueue");
const { useMcp } = await import("./store");

// React's act() wants to know it runs in a test.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const request = (id: number): ApprovalRequest => ({
  id,
  clientId: "c1",
  clientName: "claude-code",
  clientInfo: { name: "claude-code", version: "2.1.0" },
  dataSourceId: "ds",
  dataSourceName: "shop",
  dataSourceColor: "#e5484d",
  sql: "delete from orders",
  summary: "DELETE",
  writeKind: "dml",
  warnings: ["noWhereClause"],
  reason: "Remove the test orders",
  requestedAt: new Date(Date.now()).toISOString(),
  expiresAt: new Date(Date.now() + 120_000).toISOString(),
});

/** Long enough for the gate to open and the dialog to re-render. */
const ARMED = APPROVE_DELAY_MS + 20;

let root: Root;

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date", "performance"] });
  answered.length = 0;
  useMcp.setState({ approvals: emptyQueue, approvalsHidden: false });
  const host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<ApprovalDialog />));
});

afterEach(async () => {
  await act(async () => root.unmount());
  document.body.innerHTML = "";
  vi.useRealTimers();
});

async function show(...requests: ApprovalRequest[]) {
  await act(async () => {
    useMcp.setState({ approvals: requests.reduce(addRequest, emptyQueue) });
  });
}

const button = (label: string) =>
  [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === label) as HTMLButtonElement;

async function wait(ms: number) {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
}

/** Dispatches a key on the focused element; whether something prevented its default. */
async function press(key: string, target: Element = document.activeElement ?? document.body) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  await act(async () => {
    target.dispatchEvent(event);
  });
  return event.defaultPrevented;
}

async function click(target: HTMLElement, detail = 1) {
  await act(async () => {
    target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail }));
  });
}

describe("ApprovalDialog", () => {
  it("shows who asks, where, what and why", async () => {
    await show(request(1));
    const text = document.body.textContent ?? "";
    expect(text).toContain("claude-code wants to run DELETE");
    expect(text).toContain("Verified token");
    expect(text).toContain("Reported by the client, unverified: claude-code 2.1.0");
    expect(text).toContain("shop");
    expect(text).toContain("Changes data");
    expect(text).toContain("No WHERE clause");
    expect(text).toContain("Remove the test orders");
    expect(text).toContain("delete from orders");
    expect(text).toContain("Expires in 2:00");
  });

  it("focuses Reject and keeps Approve disabled for 800 ms", async () => {
    await show(request(1));
    expect(document.activeElement).toBe(button("Reject"));
    expect(button("Approve").disabled).toBe(true);
    await wait(APPROVE_DELAY_MS - 100);
    expect(button("Approve").disabled).toBe(true);
    await wait(200);
    expect(button("Approve").disabled).toBe(false);
    await click(button("Approve"));
    expect(answered).toEqual([[1, true]]);
  });

  it("swallows Enter, Space and Escape while arming, and each key restarts the wait", async () => {
    await show(request(1));
    expect(await press("Enter")).toBe(true);
    expect(await press(" ")).toBe(true);
    expect(await press("Escape")).toBe(true);
    expect(useMcp.getState().approvalsHidden).toBe(false);

    // Still typing: the gate keeps closing.
    await wait(600);
    await press("a");
    await wait(600);
    expect(button("Approve").disabled).toBe(true);
    await wait(300);
    expect(button("Approve").disabled).toBe(false);
    expect(answered).toEqual([]);
  });

  it("ignores a key-initiated Reject while arming, but not a click", async () => {
    await show(request(1));
    await click(button("Reject"), 0);
    expect(answered).toEqual([]);
    await click(button("Reject"), 1);
    expect(answered).toEqual([[1, false]]);
  });

  it("never approves on Enter", async () => {
    await show(request(1));
    await wait(ARMED);
    expect(await press("Enter", button("Approve"))).toBe(true);
    // Enter on Reject is left to the button: it rejects.
    expect(await press("Enter", button("Reject"))).toBe(false);
  });

  it("puts the dialog aside on Escape once armed, without answering", async () => {
    await show(request(1));
    await wait(ARMED);
    await press("Escape");
    expect(useMcp.getState().approvalsHidden).toBe(true);
    expect(answered).toEqual([]);
    expect(button("Approve")).toBeUndefined();
  });

  it("counts the requests and re-arms when moving to another one", async () => {
    await show(request(1), request(2));
    expect(document.body.textContent).toContain("1 of 2");
    await wait(ARMED);
    expect(button("Approve").disabled).toBe(false);

    await click(document.querySelector('[aria-label="Next request"]') as HTMLElement);
    expect(document.body.textContent).toContain("2 of 2");
    expect(button("Approve").disabled).toBe(true);
    expect(document.activeElement).toBe(button("Reject"));
    await wait(ARMED);
    await click(button("Approve"));
    expect(answered).toEqual([[2, true]]);
  });
});
