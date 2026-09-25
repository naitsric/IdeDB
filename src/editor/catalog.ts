import type { Engine } from "../db/api";
import { useDataSources } from "../db/dataSources";
import type { CompletionCatalog } from "./completion";

/** Completion's live view of a data source, read from the explorer's store at each request. */
export function catalogFor(dataSourceId: string, engine: Engine): CompletionCatalog {
  return {
    snapshot() {
      const { explorers, showSystemSchemas } = useDataSources.getState();
      const explorer = explorers[dataSourceId];
      if (explorer?.status !== "connected" || !explorer.schemas) return null;
      return {
        engine,
        defaultSchema: explorer.server?.defaultSchema ?? null,
        schemas: explorer.schemas,
        showSystemSchemas,
        models: explorer.models,
      };
    },
    loadSchema: (schema) => useDataSources.getState().loadSchema(dataSourceId, schema),
  };
}
