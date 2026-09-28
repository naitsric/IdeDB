import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { message } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { api, errorMessage, type Engine, type SessionId } from "./api";
import { activeResult, isRunning, setExecutionHooks, useConsoles, type ConsoleState } from "./consoles";
import { useDataSources } from "./dataSources";

/**
 * Transaction mode per console, like DataGrip's Tx: Auto / Manual.
 *
 * Auto: each statement commits on its own unless the user types BEGIN.
 * Manual: the first statement opens a transaction (BEGIN / START
 * TRANSACTION, run through the normal execution path so the drivers track
 * it) and everything after it, data editor submits included, stays
 * uncommitted until Commit or Rollback.
 *
 * Whether a transaction is open always comes from the drivers
 * (`ConsoleState.inTransaction`). This module adds when it started, how many
 * statements ran in it, and why it ended when the user did not end it.
 */

export type TransactionMode = "auto" | "manual";

export interface OpenTransaction {
  /** When the app first saw it open (ms since epoch). */
  since: number;
  /** Statements and data editor submits run inside it, transaction control excluded. */
  statements: number;
  /** A statement failed inside it: PostgreSQL then accepts only ROLLBACK (COMMIT rolls back). */
  failed: boolean;
}

interface TransactionsState {
  modes: Record<string, TransactionMode>;
  open: Record<string, OpenTransaction>;
  /** Why a console's transaction ended without the user ending it, or why ending it failed. */
  notices: Record<string, string>;
  /** Commit or rollback in progress. */
  ending: Record<string, boolean>;
}

/* Pure rules, exported for tests. */

export function beginSql(engine: Engine): string {
  return engine === "mysql" ? "start transaction" : "begin";
}

const LEADING_COMMENTS = /^(\s*(--[^\n]*(\n|$)|\/\*[\s\S]*?\*\/))*\s*/;
const CONTROL = /^(begin|start\s+transaction|commit|end|rollback|abort|savepoint|release)\b/i;

/** BEGIN, COMMIT, ROLLBACK, SAVEPOINT and friends: statements that manage the transaction itself. */
export function isTransactionControl(sql: string): boolean {
  return CONTROL.test(sql.replace(LEADING_COMMENTS, ""));
}

/** Whether a statement must first open a transaction: manual mode, none open, user SQL. */
export function needsBegin(mode: TransactionMode, inTransaction: boolean, sql: string, tableLoad: boolean): boolean {
  return mode === "manual" && !inTransaction && !tableLoad && !isTransactionControl(sql);
}

/**
 * The transaction's bookkeeping after a statement, and a notice when it
 * ended without the user ending it (lost connection, an error that rolled
 * it back, an implicit commit).
 */
export function nextTransactionState(
  prev: OpenTransaction | undefined,
  o: { wasOpen: boolean; nowOpen: boolean; sql: string; error?: string; sessionLost: boolean; engine?: Engine; at: number },
): { open?: OpenTransaction; notice?: string } {
  const control = isTransactionControl(o.sql);
  if (o.nowOpen) {
    const base = prev ?? { since: o.at, statements: 0, failed: false };
    // A successful ROLLBACK TO SAVEPOINT recovers a failed PostgreSQL transaction.
    const failed = control && o.error === undefined ? false : base.failed || (o.error !== undefined && o.engine === "postgres");
    return { open: { since: base.since, statements: base.statements + (control ? 0 : 1), failed } };
  }
  if (!o.wasOpen) return {};
  if (o.sessionLost) return { notice: "The session was lost and the open transaction with it: nothing in it was committed." };
  if (o.error !== undefined) return { notice: `The open transaction ended: ${o.error.replace(/\s+/g, " ").trim()}` };
  if (control) return {};
  return {
    notice: "This statement ended the open transaction: the database committed it implicitly (MySQL does for DDL, for example).",
  };
}

/** Time a transaction has been open, as a clock: `0:07`, `12:40`, `1:02:09`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const [h, m, s] = [Math.floor(total / 3600), Math.floor(total / 60) % 60, total % 60];
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** Consoles whose session has a transaction open, optionally only those of one data source. */
export function consolesWithOpenTransactions(consoles: Record<string, ConsoleState>, dataSourceId?: string): string[] {
  return Object.values(consoles)
    .filter((c) => c.inTransaction && (dataSourceId === undefined || c.dataSourceId === dataSourceId))
    .map((c) => c.id);
}

/* State. */

const MODES_KEY = "idedb.transactionModes.v1";

function restoreModes(): Record<string, TransactionMode> {
  try {
    const saved = JSON.parse(localStorage.getItem(MODES_KEY) ?? "{}") as Record<string, unknown>;
    return Object.fromEntries(Object.entries(saved).filter(([, mode]) => mode === "manual")) as Record<string, TransactionMode>;
  } catch {
    return {};
  }
}

function persistModes(modes: Record<string, TransactionMode>) {
  try {
    localStorage.setItem(MODES_KEY, JSON.stringify(modes));
  } catch {
    // Mode not persisted; the console starts in Auto next launch.
  }
}

export const useTransactions = create<TransactionsState>(() => ({ modes: restoreModes(), open: {}, notices: {}, ending: {} }));

const without = <T>(record: Record<string, T>, key: string): Record<string, T> => {
  const { [key]: _, ...rest } = record;
  return rest;
};

export const modeOf = (consoleId: string): TransactionMode => useTransactions.getState().modes[consoleId] ?? "auto";

export function setMode(consoleId: string, mode: TransactionMode) {
  const modes = mode === "manual" ? { ...useTransactions.getState().modes, [consoleId]: mode } : without(useTransactions.getState().modes, consoleId);
  useTransactions.setState({ modes });
  persistModes(modes);
}

export function toggleMode(consoleId: string) {
  setMode(consoleId, modeOf(consoleId) === "manual" ? "auto" : "manual");
}

function setNotice(consoleId: string, notice: string | undefined) {
  const { notices } = useTransactions.getState();
  useTransactions.setState({ notices: notice ? { ...notices, [consoleId]: notice } : without(notices, consoleId) });
}

export const dismissNotice = (consoleId: string) => setNotice(consoleId, undefined);

function setEnding(consoleId: string, ending: boolean) {
  const current = useTransactions.getState().ending;
  useTransactions.setState({ ending: ending ? { ...current, [consoleId]: true } : without(current, consoleId) });
}

const engineOf = (entry: ConsoleState): Engine | undefined =>
  useDataSources.getState().sources.find((s) => s.id === entry.dataSourceId)?.params.engine;

/* Running transaction control. */

/**
 * Runs BEGIN/COMMIT/ROLLBACK on a console's session outside the result tabs.
 * The session closes its open result before running anything, so the
 * console's results stop offering more rows first.
 */
async function runControl(
  consoleId: string,
  sessionId: SessionId,
  sql: string,
): Promise<{ inTransaction: boolean; error?: string }> {
  useConsoles.getState().releaseOpenResults(consoleId);
  let outcome: { inTransaction: boolean; error?: string } = { inTransaction: false, error: "No answer from the database." };
  try {
    // Transaction control returns no rows: nothing to page.
    await api.execute(sessionId, sql, null, (event) => {
      if (event.kind === "done") outcome = { inTransaction: event.inTransaction };
      else if (event.kind === "error") outcome = { inTransaction: event.inTransaction, error: event.message };
    });
  } catch (e) {
    return { inTransaction: false, error: errorMessage(e) };
  }
  return outcome;
}

/**
 * In manual mode, opens a transaction on the console's session before a
 * statement or a data editor submit runs. Resolves to an error message when
 * it could not, so the caller does not write outside a transaction.
 */
export async function beginIfManual(
  consoleId: string,
  sessionId: SessionId,
  sql = "",
  tableLoad = false,
): Promise<string | undefined> {
  const entry = useConsoles.getState().consoles[consoleId];
  if (!entry || !needsBegin(modeOf(consoleId), entry.inTransaction ?? false, sql, tableLoad)) return undefined;
  const engine = engineOf(entry);
  if (!engine) return undefined;
  const outcome = await runControl(consoleId, sessionId, beginSql(engine));
  useConsoles.getState().setInTransaction(consoleId, outcome.inTransaction);
  if (outcome.error !== undefined) return `Could not open a transaction (manual mode): ${outcome.error}`;
  if (!outcome.inTransaction) return "Could not open a transaction (manual mode): the session still commits each statement.";
  setNotice(consoleId, undefined);
  return undefined;
}

/** Counts a data editor submit that went into the open transaction. */
export function recordSubmit(consoleId: string) {
  const open = useTransactions.getState().open[consoleId];
  if (open) useTransactions.setState((s) => ({ open: { ...s.open, [consoleId]: { ...open, statements: open.statements + 1 } } }));
}

/**
 * Commits or rolls back the console's open transaction. Resolves to whether
 * it is closed afterwards; a failure is shown as the console's notice.
 */
export async function endTransaction(consoleId: string, kind: "commit" | "rollback"): Promise<boolean> {
  const entry = useConsoles.getState().consoles[consoleId];
  if (!entry?.inTransaction || entry.sessionId === undefined) return true;
  if (useTransactions.getState().ending[consoleId]) return false;
  const failedBefore = useTransactions.getState().open[consoleId]?.failed ?? false;

  setEnding(consoleId, true);
  try {
    // Only reachable from the close guards: the buttons are disabled while a statement runs.
    if (isRunning(entry)) await useConsoles.getState().cancel(consoleId);
    const outcome = await runControl(consoleId, entry.sessionId, kind);
    useConsoles.getState().setInTransaction(consoleId, outcome.inTransaction);
    if (outcome.error !== undefined) {
      setNotice(consoleId, `${kind === "commit" ? "Commit" : "Rollback"} failed: ${outcome.error}`);
      return false;
    }
    setNotice(
      consoleId,
      kind === "commit" && failedBefore && engineOf(entry) === "postgres"
        ? "PostgreSQL rolled the transaction back instead of committing it: a statement in it had failed."
        : undefined,
    );
    // Rows the data editor shows may include changes that were just undone.
    if (kind === "rollback" || failedBefore) reloadTableData(consoleId);
    return !outcome.inTransaction;
  } finally {
    setEnding(consoleId, false);
  }
}

function reloadTableData(consoleId: string) {
  const result = activeResult(useConsoles.getState().consoles[consoleId]);
  if (result?.table && result.status !== "running") {
    void useConsoles.getState().runStatement(consoleId, result.sql, { table: result.table });
  }
}

/* Guard rails. */

function consoleTitle(entry: ConsoleState): string {
  return entry.table?.name ?? useDataSources.getState().sources.find((s) => s.id === entry.dataSourceId)?.name ?? "console";
}

/**
 * Before something that would end the consoles' sessions (closing a tab,
 * disconnecting, deleting a data source, quitting), asks whether to commit
 * or roll back their open transactions. Resolves to whether to go on.
 */
export async function confirmEndTransactions(consoleIds: readonly string[], action: string): Promise<boolean> {
  const { consoles } = useConsoles.getState();
  const open = consoleIds.filter((id) => consoles[id]?.inTransaction);
  if (open.length === 0) return true;

  const statements = (id: string) => useTransactions.getState().open[id]?.statements ?? 0;
  const text =
    open.length === 1
      ? `The console "${consoleTitle(consoles[open[0]])}" has an open transaction (${statements(open[0])} ${statements(open[0]) === 1 ? "statement" : "statements"}).`
      : `${open.length} consoles have open transactions: ${open.map((id) => `"${consoleTitle(consoles[id])}"`).join(", ")}.`;
  const choice = await message(`${text}\n\nCommit or roll back before you ${action}?`, {
    title: open.length === 1 ? "Open transaction" : "Open transactions",
    kind: "warning",
    buttons: { yes: "Commit", no: "Rollback", cancel: "Cancel" },
  });
  if (choice !== "Commit" && choice !== "Rollback") return false;

  const kind = choice === "Commit" ? "commit" : "rollback";
  const ended = await Promise.all(open.map((id) => endTransaction(id, kind)));
  return ended.every(Boolean);
}

/** Quits through the guard; the native Quit item and ⌘Q come here. */
export async function quitApp() {
  const open = consolesWithOpenTransactions(useConsoles.getState().consoles);
  if (await confirmEndTransactions(open, "quit IdeDB")) await invoke("app_quit");
}

/** Asks before the window closes with transactions open. Returns the unsubscribe function. */
export function guardWindowClose(): () => void {
  let unlisten: (() => void) | undefined;
  let disposed = false;
  void getCurrentWindow()
    .onCloseRequested(async (event) => {
      const open = consolesWithOpenTransactions(useConsoles.getState().consoles);
      if (open.length === 0) return;
      event.preventDefault();
      if (await confirmEndTransactions(open, "close IdeDB")) await getCurrentWindow().destroy();
    })
    .then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/* Wiring. */

setExecutionHooks({
  before: (consoleId, sessionId, sql, tableLoad) => beginIfManual(consoleId, sessionId, sql, tableLoad),
  after: (consoleId, sql, outcome) => {
    const entry = useConsoles.getState().consoles[consoleId];
    const next = nextTransactionState(useTransactions.getState().open[consoleId], {
      ...outcome,
      sql,
      engine: entry ? engineOf(entry) : undefined,
      at: Date.now(),
    });
    if (next.open) useTransactions.setState((s) => ({ open: { ...s.open, [consoleId]: next.open! } }));
    if (next.notice) setNotice(consoleId, next.notice);
    else if (!outcome.wasOpen && outcome.nowOpen) setNotice(consoleId, undefined);
  },
});

// Keep the bookkeeping in step with what the drivers report, whatever the
// path (statements, data editor submits, disconnects), and forget consoles
// that are gone.
useConsoles.subscribe(({ consoles }) => {
  const state = useTransactions.getState();
  let { open, modes, notices } = state;
  for (const [id, entry] of Object.entries(consoles)) {
    if (entry.inTransaction && !open[id]) open = { ...open, [id]: { since: Date.now(), statements: 0, failed: false } };
    if (!entry.inTransaction && open[id]) open = without(open, id);
  }
  for (const id of Object.keys(open)) if (!consoles[id]) open = without(open, id);
  for (const id of Object.keys(notices)) if (!consoles[id]) notices = without(notices, id);
  const staleModes = Object.keys(modes).filter((id) => !consoles[id]);
  for (const id of staleModes) modes = without(modes, id);
  if (open !== state.open || notices !== state.notices || modes !== state.modes) {
    useTransactions.setState({ open, notices, modes });
    if (modes !== state.modes) persistModes(modes);
  }
});
