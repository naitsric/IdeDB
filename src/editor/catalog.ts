import type { Engine } from "../db/api";
import { effectiveSchema, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import type { CompletionCatalog } from "./completion";

/**
 * Completion's live view of a console's data source, read from the
 * explorer's store at each request. Unqualified names resolve in the
 * console's current schema.
 */
export function catalogFor(consoleId: string, dataSourceId: string, engine: Engine): CompletionCatalog {
  return {
    snapshot() {
      const { explorers, showSystemSchemas } = useDataSources.getState();
      const explorer = explorers[dataSourceId];
      if (explorer?.status !== "connected" || !explorer.schemas) return null;
      return {
        engine,
        defaultSchema: effectiveSchema(useConsoles.getState().consoles[consoleId]),
        schemas: explorer.schemas,
        showSystemSchemas,
        models: explorer.models,
      };
    },
    loadSchema: (schema) => useDataSources.getState().loadSchema(dataSourceId, schema),
  };
}
