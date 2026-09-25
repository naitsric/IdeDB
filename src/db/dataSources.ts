import { create } from "zustand";
import {
  api,
  errorMessage,
  isCommandError,
  type DataSource,
  type SchemaInfo,
  type SchemaModel,
  type ServerInfo,
  type SessionId,
} from "./api";
import { requestPassword } from "../dialogs/dialogs";
// Circular with consoles.ts; safe because each side only uses the other inside functions.
import { useConsoles } from "./consoles";

/**
 * Saved data sources and, for connected ones, the explorer's view of them.
 * The explorer uses its own session per data source, separate from
 * consoles, so introspection never queues behind a running query.
 */

export type SchemaLoad =
  | { state: "loading" }
  | { state: "loaded"; model: SchemaModel }
  | { state: "error"; error: string };

export interface ExplorerState {
  status: "connecting" | "connected" | "error";
  error?: string;
  sessionId?: SessionId;
  server?: ServerInfo;
  schemas?: SchemaInfo[];
  models: Record<string, SchemaLoad>;
}

interface DataSourcesState {
  sources: DataSource[];
  loaded: boolean;
  explorers: Record<string, ExplorerState>;
  showSystemSchemas: boolean;

  load: () => Promise<void>;
  save: (source: DataSource, password?: string) => Promise<DataSource>;
  remove: (id: string) => Promise<void>;
  connect: (id: string) => Promise<void>;
  disconnect: (id: string) => Promise<void>;
  refresh: (id: string) => Promise<void>;
  /** Resolves once the schema's model is loaded (or failed). Concurrent calls share one request. */
  loadSchema: (id: string, schema: string, options?: { reload?: boolean }) => Promise<void>;
  toggleSystemSchemas: () => void;
}

/**
 * Background introspection after connecting, so completion and Go to Table
 * know every table, as DataGrip does. The explorer's session runs one
 * introspection at a time anyway; two in flight keeps it busy without
 * queueing a user's expand behind many background loads.
 */
const BACKGROUND_CONCURRENCY = 2;
/** Past this many schemas, the rest load on demand (expanding one, or completing `schema.`). */
const BACKGROUND_SCHEMA_LIMIT = 100;

/** In-flight introspections, so explorer, completion and background loads share one request. */
const schemaLoads = new Map<string, Promise<void>>();

/**
 * Passwords typed at a prompt, kept in memory until the app quits so every
 * console of the same data source connects without asking again.
 */
const sessionPasswords = new Map<string, string>();

/**
 * Opens a session for a data source, prompting for the password when it is
 * not saved. Resolves to `null` if the user dismisses the prompt.
 */
export async function openSessionFor(dataSourceId: string) {
  try {
    return await api.openSession(dataSourceId, sessionPasswords.get(dataSourceId));
  } catch (e) {
    const source = useDataSources.getState().sources.find((s) => s.id === dataSourceId);
    if (!isCommandError(e, "passwordRequired") || !source) throw e;
    const password = await requestPassword(source);
    if (password === null) return null;
    const opened = await api.openSession(dataSourceId, password);
    sessionPasswords.set(dataSourceId, password);
    return opened;
  }
}

export const useDataSources = create<DataSourcesState>((set, get) => {
  const patchExplorer = (id: string, patch: Partial<ExplorerState>) =>
    set((s) => {
      const current = s.explorers[id];
      if (!current) return {};
      return { explorers: { ...s.explorers, [id]: { ...current, ...patch } } };
    });

  const setModel = (id: string, schema: string, load: SchemaLoad, sessionId?: SessionId) =>
    set((s) => {
      const current = s.explorers[id];
      // Ignore results from a session that has since been closed or replaced.
      if (!current || (sessionId !== undefined && current.sessionId !== sessionId)) return {};
      return {
        explorers: { ...s.explorers, [id]: { ...current, models: { ...current.models, [schema]: load } } },
      };
    });

  /** Introspects the data source's remaining non-system schemas, a few at a time. */
  async function loadInBackground(id: string) {
    const explorer = get().explorers[id];
    if (!explorer?.schemas) return;
    const queue = explorer.schemas
      .filter((schema) => !schema.isSystem && !explorer.models[schema.name])
      .slice(0, BACKGROUND_SCHEMA_LIMIT)
      .map((schema) => schema.name);
    const worker = async () => {
      for (let schema = queue.shift(); schema !== undefined; schema = queue.shift()) {
        // Stop if the data source was disconnected or reconnected meanwhile.
        if (get().explorers[id]?.sessionId !== explorer.sessionId) return;
        await get().loadSchema(id, schema);
      }
    };
    await Promise.all(Array.from({ length: BACKGROUND_CONCURRENCY }, worker));
  }

  return {
    sources: [],
    loaded: false,
    explorers: {},
    showSystemSchemas: false,

    load: async () => {
      set({ sources: await api.listDataSources(), loaded: true });
    },

    save: async (source, password) => {
      const saved = await api.saveDataSource(source, password);
      if (password !== undefined) sessionPasswords.set(saved.id, password);
      set((s) => {
        const exists = s.sources.some((x) => x.id === saved.id);
        return { sources: exists ? s.sources.map((x) => (x.id === saved.id ? saved : x)) : [...s.sources, saved] };
      });
      return saved;
    },

    remove: async (id) => {
      await get().disconnect(id);
      await api.deleteDataSource(id);
      sessionPasswords.delete(id);
      set((s) => ({ sources: s.sources.filter((x) => x.id !== id) }));
    },

    connect: async (id) => {
      const existing = get().explorers[id];
      if (existing?.status === "connecting" || existing?.status === "connected") return;
      set((s) => ({ explorers: { ...s.explorers, [id]: { status: "connecting", models: {} } } }));
      try {
        const opened = await openSessionFor(id);
        if (!opened) {
          set((s) => {
            const { [id]: _, ...rest } = s.explorers;
            return { explorers: rest };
          });
          return;
        }
        patchExplorer(id, { status: "connected", sessionId: opened.id, server: opened.server });
        const schemas = await api.schemas(opened.id);
        patchExplorer(id, { schemas });
        const preferred = opened.server.defaultSchema ?? schemas.find((x) => !x.isSystem)?.name;
        if (preferred) await get().loadSchema(id, preferred);
        void loadInBackground(id);
      } catch (e) {
        patchExplorer(id, { status: "error", error: errorMessage(e) });
      }
    },

    disconnect: async (id) => {
      const explorer = get().explorers[id];
      set((s) => {
        const { [id]: _, ...rest } = s.explorers;
        return { explorers: rest };
      });
      await useConsoles.getState().disconnectDataSource(id);
      if (explorer?.sessionId !== undefined) await api.closeSession(explorer.sessionId).catch(() => {});
    },

    refresh: async (id) => {
      const explorer = get().explorers[id];
      if (explorer?.status !== "connected" || explorer.sessionId === undefined) {
        await get().connect(id);
        return;
      }
      try {
        patchExplorer(id, { schemas: await api.schemas(explorer.sessionId) });
        const loaded = Object.keys(explorer.models);
        await Promise.all(loaded.map((schema) => get().loadSchema(id, schema, { reload: true })));
        void loadInBackground(id);
      } catch (e) {
        patchExplorer(id, { status: "error", error: errorMessage(e) });
      }
    },

    loadSchema: (id, schema, { reload = false } = {}) => {
      const sessionId = get().explorers[id]?.sessionId;
      if (sessionId === undefined) return Promise.resolve();
      if (!reload && get().explorers[id]?.models[schema]?.state === "loaded") return Promise.resolve();

      const key = `${sessionId}\0${schema}`;
      const inFlight = schemaLoads.get(key);
      if (inFlight) return inFlight;

      if (!get().explorers[id]?.models[schema]) setModel(id, schema, { state: "loading" });
      const load = api
        .introspect(sessionId, schema)
        .then(
          (model) => setModel(id, schema, { state: "loaded", model }, sessionId),
          (e) => setModel(id, schema, { state: "error", error: errorMessage(e) }, sessionId),
        )
        .finally(() => schemaLoads.delete(key));
      schemaLoads.set(key, load);
      return load;
    },

    toggleSystemSchemas: () => set((s) => ({ showSystemSchemas: !s.showSystemSchemas })),
  };
});
