import { listen } from "@tauri-apps/api/event";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { errorMessage } from "../db/api";
import { mcpApi, MCP_EVENT, type McpClient, type McpEvent, type McpSettings } from "./api";
import { addPending, emptyQueue, resolveRequest, stepRequest } from "./approvalsQueue";
import { applyEvent, AUDIT_RING, mergeAudit, type McpSnapshot } from "./events";
import { withGrant, type GrantLevel } from "./grants";

/**
 * The MCP server as the UI sees it: its status and settings, the clients
 * and their grants, writes waiting for approval and the latest activity.
 * Kept current by `mcp://event`; see {@link initMcp}.
 */

export type McpTab = "clients" | "server";

export type McpDialog =
  | { kind: "newClient" }
  /** A token just created or regenerated: shown once. */
  | { kind: "token"; client: McpClient; token: string; regenerated: boolean }
  | { kind: "rename"; client: McpClient };

interface McpState extends McpSnapshot {
  loaded: boolean;
  /** Why the initial load failed. */
  loadError: string | null;
  settings: McpSettings | null;
  /** Ids of the data sources marked never-write. */
  neverWrite: string[];
  tab: McpTab;
  selectedClientId: string | null;
  dialog: McpDialog | null;
}

export const useMcp = create<McpState>(() => ({
  loaded: false,
  loadError: null,
  status: { running: false, port: null, url: null, error: null },
  settings: null,
  clients: [],
  neverWrite: [],
  approvals: emptyQueue,
  approvalsHidden: false,
  audit: [],
  tab: "clients",
  selectedClientId: null,
  dialog: null,
}));

const set = useMcp.setState;
const get = useMcp.getState;

/** Status events seen, so a status read that an event overtook is not applied after it. */
let statusEvents = 0;

function onEvent(event: McpEvent) {
  if (event.kind === "status") statusEvents++;
  set((state) => applyEvent(state, event));
}

/**
 * Listens to the server's events, then loads its state. Writes waiting
 * for approval are read too, so a reloaded UI asks about them again. Call
 * once at app start; returns the cleanup.
 */
export function initMcp(): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  void listen<McpEvent>(MCP_EVENT, ({ payload }) => onEvent(payload)).then((stop) => {
    if (disposed) return stop();
    unlisten = stop;
    void loadMcp();
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/**
 * Reads everything again; events that arrive meanwhile are kept. Each part
 * stands alone, so pending approvals still show if, say, the audit log
 * can't be read.
 */
export async function loadMcp() {
  const seen = statusEvents;
  const [status, settings, clients, neverWrite, pending, audit] = await Promise.allSettled([
    mcpApi.status(),
    mcpApi.settings(),
    mcpApi.clients(),
    mcpApi.neverWrite(),
    mcpApi.pendingApprovals(),
    mcpApi.audit({ limit: AUDIT_RING }),
  ]);
  const ok = <T>(result: PromiseSettledResult<T>) => (result.status === "fulfilled" ? result.value : undefined);
  const failed = [status, settings, clients, neverWrite, pending, audit].find(
    (r): r is PromiseRejectedResult => r.status === "rejected",
  );
  set((state) => ({
    loaded: true,
    loadError: failed ? errorMessage(failed.reason) : null,
    status: (statusEvents === seen && ok(status)) || state.status,
    settings: ok(settings) ?? state.settings,
    clients: ok(clients) ?? state.clients,
    neverWrite: ok(neverWrite) ?? state.neverWrite,
    approvals: addPending(state.approvals, ok(pending) ?? []),
    audit: mergeAudit(state.audit, ok(audit) ?? []),
  }));
}

/** The clients again, e.g. when the tool window opens: `lastSeenAt` moves without events in between. */
export async function reloadClients() {
  try {
    const [clients, neverWrite] = await Promise.all([mcpApi.clients(), mcpApi.neverWrite()]);
    set({ clients, neverWrite });
  } catch {
    // The list shown stays; the next action reports what is wrong.
  }
}

async function showError(title: string, e: unknown) {
  await message(errorMessage(e), { title, kind: "error" });
}

// Server

/** Saves the settings; the server starts, restarts or stops to match. Throws what the server refused. */
export async function saveSettings(settings: McpSettings) {
  const seen = statusEvents;
  const status = await mcpApi.saveSettings(settings);
  set(statusEvents === seen ? { settings, status } : { settings });
}

/** Turns the server on or off, keeping the other settings as saved. */
export async function toggleServer() {
  try {
    const settings = get().settings ?? (await mcpApi.settings());
    await saveSettings({ ...settings, enabled: !settings.enabled });
  } catch (e) {
    await showError("Couldn't change the MCP server", e);
  }
}

/** Starts the server again with the saved settings, e.g. after it failed to start. */
export async function retryServer() {
  try {
    await saveSettings(get().settings ?? (await mcpApi.settings()));
  } catch (e) {
    await showError("Couldn't start the MCP server", e);
  }
}

// Clients

export function selectClient(id: string | null) {
  set({ selectedClientId: id });
}

export function setMcpTab(tab: McpTab) {
  set({ tab });
}

export function openNewClient() {
  set({ dialog: { kind: "newClient" } });
}

export function openRename(client: McpClient) {
  set({ dialog: { kind: "rename", client } });
}

export function closeDialog() {
  set({ dialog: null });
}

const replaceClient = (client: McpClient) =>
  set((state) => ({ clients: state.clients.map((c) => (c.id === client.id ? client : c)) }));

/** Registers a client and shows its token. Throws what the server refused. */
export async function createClient(name: string) {
  const { client, token } = await mcpApi.createClient(name);
  set((state) => ({
    clients: [...state.clients, client],
    selectedClientId: client.id,
    tab: "clients",
    dialog: { kind: "token", client, token, regenerated: false },
  }));
}

/** Throws what the server refused. */
export async function renameClient(id: string, name: string) {
  replaceClient(await mcpApi.renameClient(id, name));
}

export async function regenerateToken(client: McpClient) {
  const confirmed = await ask(
    "The current token stops working at once, and any write it sent that waits for approval is withdrawn. " +
      "The client needs the new token to connect again.",
    { title: `Regenerate the token of "${client.name}"?`, kind: "warning", okLabel: "Regenerate" },
  );
  if (!confirmed) return;
  try {
    const { client: rotated, token } = await mcpApi.rotateClient(client.id);
    replaceClient(rotated);
    set({ dialog: { kind: "token", client: rotated, token, regenerated: true } });
  } catch (e) {
    await showError("Couldn't regenerate the token", e);
  }
}

export async function revokeClient(client: McpClient) {
  const confirmed = await ask(
    "Its token stops working at once, and any write it sent that waits for approval is withdrawn. " +
      "It stays in the list, and its activity in the log.",
    { title: `Revoke "${client.name}"?`, kind: "warning", okLabel: "Revoke" },
  );
  if (!confirmed) return;
  try {
    replaceClient(await mcpApi.revokeClient(client.id));
  } catch (e) {
    await showError("Couldn't revoke the client", e);
  }
}

export async function deleteClient(client: McpClient) {
  const confirmed = await ask(
    "Its token stops working at once and its access is removed. Its activity stays in the log.",
    { title: `Delete "${client.name}"?`, kind: "warning", okLabel: "Delete" },
  );
  if (!confirmed) return;
  try {
    await mcpApi.deleteClient(client.id);
    set((state) => ({
      clients: state.clients.filter((c) => c.id !== client.id),
      selectedClientId: state.selectedClientId === client.id ? null : state.selectedClientId,
    }));
  } catch (e) {
    await showError("Couldn't delete the client", e);
  }
}

/** Sets a client's access to one data source, showing it at once and undoing it if the save fails. */
export async function setAccess(clientId: string, dataSourceId: string, level: GrantLevel) {
  const before = get().clients.find((c) => c.id === clientId);
  if (!before) return;
  const grants = withGrant(before.grants, dataSourceId, level);
  replaceClient({ ...before, grants });
  try {
    replaceClient(await mcpApi.setGrants(clientId, grants));
  } catch (e) {
    replaceClient(before);
    await showError("Couldn't change the client's access", e);
  }
}

/** Marks a data source never-write for every client, or clears it. */
export async function setNeverWrite(dataSourceId: string, value: boolean) {
  const before = get().neverWrite;
  const others = before.filter((id) => id !== dataSourceId);
  set({ neverWrite: value ? [...others, dataSourceId] : others });
  try {
    await mcpApi.setNeverWrite(dataSourceId, value);
  } catch (e) {
    set({ neverWrite: before });
    await showError("Couldn't change never write", e);
  }
}

// Approvals

/**
 * Answers the request and takes it off the queue: answered now, or
 * already resolved (timed out, withdrawn) with its event on the way.
 */
export async function answerApproval(id: number, approve: boolean) {
  try {
    await mcpApi.answerApproval(id, approve);
    set((state) => ({ approvals: resolveRequest(state.approvals, id) }));
  } catch (e) {
    await showError("Couldn't answer the approval", e);
  }
}

export function showApprovals() {
  set({ approvalsHidden: false });
}

/** Puts the dialog aside; the requests keep waiting, and the status bar brings them back. */
export function hideApprovals() {
  set({ approvalsHidden: true });
}

export function stepApproval(step: -1 | 1) {
  set((state) => ({ approvals: stepRequest(state.approvals, step) }));
}
