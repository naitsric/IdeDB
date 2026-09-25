import type { ColumnInfo, DataSource, Engine, ObjectKind } from "../db/api";
import type { ExplorerState } from "../db/dataSources";

/**
 * Explorer tree model: data source → schema → tables/views → columns.
 * Children are built lazily: an unloaded level is an empty array (so it
 * shows an expand arrow) until the user opens it.
 */
export interface TreeNode {
  id: string;
  name: string;
  kind: "dataSource" | "schema" | "group" | "table" | "column" | "message";
  /** Muted text after the name. */
  detail?: string;
  sourceId: string;
  schema?: string;
  table?: string;
  engine?: Engine;
  color?: string | null;
  status?: ExplorerState["status"];
  objectKind?: ObjectKind;
  column?: ColumnInfo;
  isForeignKey?: boolean;
  tone?: "loading" | "error";
  /** `undefined` = leaf. */
  children?: TreeNode[];
}

export function buildTree(
  sources: DataSource[],
  explorers: Record<string, ExplorerState>,
  showSystemSchemas: boolean,
  openTables: ReadonlySet<string>,
): TreeNode[] {
  return sources.map((source) => {
    const id = `ds:${source.id}`;
    const explorer = explorers[source.id];
    return {
      id,
      name: source.name,
      kind: "dataSource",
      detail: explorer?.server?.version,
      sourceId: source.id,
      engine: source.params.engine,
      color: source.color,
      status: explorer?.status,
      children: dataSourceChildren(id, source.id, explorer, showSystemSchemas, openTables),
    };
  });
}

function dataSourceChildren(
  parentId: string,
  sourceId: string,
  explorer: ExplorerState | undefined,
  showSystemSchemas: boolean,
  openTables: ReadonlySet<string>,
): TreeNode[] {
  if (!explorer) return [];
  if (explorer.status === "connecting" || (explorer.status === "connected" && !explorer.schemas)) {
    return [message(parentId, sourceId, "Connecting…", "loading")];
  }
  if (explorer.status === "error") return [message(parentId, sourceId, explorer.error ?? "Connection failed", "error")];

  const defaultSchema = explorer.server?.defaultSchema;
  const schemas = (explorer.schemas ?? [])
    .filter((s) => showSystemSchemas || !s.isSystem || s.name === defaultSchema)
    .sort((a, b) => Number(b.name === defaultSchema) - Number(a.name === defaultSchema));

  return schemas.map((schema) => {
    const id = `sc:${sourceId}/${schema.name}`;
    return {
      id,
      name: schema.name,
      kind: "schema",
      detail: schema.name === defaultSchema ? "default" : undefined,
      sourceId,
      schema: schema.name,
      children: schemaChildren(id, sourceId, schema.name, explorer, openTables),
    };
  });
}

function schemaChildren(
  parentId: string,
  sourceId: string,
  schema: string,
  explorer: ExplorerState,
  openTables: ReadonlySet<string>,
): TreeNode[] {
  const load = explorer.models[schema];
  if (!load) return [];
  if (load.state === "loading") return [message(parentId, sourceId, "Loading…", "loading")];
  if (load.state === "error") return [message(parentId, sourceId, load.error, "error")];

  const tables = load.model.tables.filter((t) => t.kind === "table" || t.kind === "foreignTable");
  const views = load.model.tables.filter((t) => t.kind === "view" || t.kind === "materializedView");
  const groups: TreeNode[] = [];
  for (const [label, items] of [["tables", tables], ["views", views]] as const) {
    if (items.length === 0) continue;
    const groupId = `gr:${sourceId}/${schema}/${label}`;
    groups.push({
      id: groupId,
      name: label,
      kind: "group",
      detail: String(items.length),
      sourceId,
      schema,
      children: items.map((table) => {
        const tableId = `tb:${sourceId}/${schema}/${table.name}`;
        const fkColumns = new Set(table.foreignKeys.flatMap((fk) => fk.columns));
        return {
          id: tableId,
          name: table.name,
          kind: "table",
          sourceId,
          schema,
          table: table.name,
          objectKind: table.kind,
          children: openTables.has(tableId)
            ? table.columns.map((column) => ({
                id: `co:${sourceId}/${schema}/${table.name}/${column.name}`,
                name: column.name,
                kind: "column",
                detail: column.typeName,
                sourceId,
                schema,
                table: table.name,
                column,
                isForeignKey: fkColumns.has(column.name),
              }))
            : [],
        };
      }),
    });
  }
  return groups;
}

function message(parentId: string, sourceId: string, text: string, tone: TreeNode["tone"]): TreeNode {
  return { id: `msg:${parentId}`, name: text, kind: "message", sourceId, tone };
}
