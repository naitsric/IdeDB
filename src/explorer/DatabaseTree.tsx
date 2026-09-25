import {
  ChevronRight,
  CircleAlert,
  Columns3,
  Eye,
  Folder,
  KeyRound,
  Link2,
  LoaderCircle,
  Search,
  Table2,
} from "lucide-react";
import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Tree, type NodeApi, type NodeRendererProps, type TreeApi } from "react-arborist";
import { openTableData } from "../actions";
import { useDataSources } from "../db/dataSources";
import {
  CommandItem,
  ContextMenuContent,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "../ui/ContextMenu";
import { EngineIcon } from "../ui/EngineIcon";
import { cx, StatusDot } from "../ui/primitives";
import { useElementSize } from "../ui/useElementSize";
import { useExplorerSelection, type ExplorerSelection } from "./selection";
import { buildTree, type TreeNode } from "./treeData";

const ROW_HEIGHT = 24;
const STATUS_TONE = { connected: "success", connecting: "warning", error: "danger" } as const;

export function DatabaseTree() {
  const { sources, explorers, showSystemSchemas, connect, loadSchema } = useDataSources();
  const [openTables, setOpenTables] = useState<ReadonlySet<string>>(new Set());
  const [query, setQuery] = useState("");
  const [menuNode, setMenuNode] = useState<TreeNode | null>(null);
  const tree = useRef<TreeApi<TreeNode> | null>(null);
  const { ref: container, width, height } = useElementSize<HTMLDivElement>();

  const data = useMemo(
    () => buildTree(sources, explorers, showSystemSchemas, openTables),
    [sources, explorers, showSystemSchemas, openTables],
  );

  const onToggle = (id: string) => {
    const node = tree.current?.get(id);
    if (!node) return;
    const d = node.data;
    const open = node.isOpen;
    if (open && d.kind === "dataSource" && !explorers[d.sourceId]) void connect(d.sourceId);
    if (open && d.kind === "schema" && !explorers[d.sourceId]?.models[d.schema!]) void loadSchema(d.sourceId, d.schema!);
    if (d.kind === "table") {
      setOpenTables((prev) => {
        const next = new Set(prev);
        if (open) next.add(id);
        else next.delete(id);
        return next;
      });
    }
  };

  const activate = (node: NodeApi<TreeNode>) => {
    const d = node.data;
    if (d.kind === "table") openTableData(d.sourceId, d.schema!, d.table!);
    else if (!node.isLeaf) node.toggle();
  };

  /**
   * Speed search, as in IntelliJ: typing while the tree has focus filters it
   * to matches; Backspace edits the query, Esc clears it. Handled in capture
   * so arborist's own single-letter shortcuts never see the keystrokes.
   */
  const onKeyDownCapture = (e: KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey || e.altKey || (e.target as HTMLElement).closest("input")) return;
    const consume = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (e.key === "Escape" && query) {
      consume();
      setQuery("");
    } else if (e.key === "Backspace" && query) {
      consume();
      setQuery((q) => q.slice(0, -1));
    } else if (e.key === "Enter") {
      const focused = tree.current?.focusedNode;
      if (focused) {
        consume();
        activate(focused);
      }
    } else if (e.key.length === 1 && (query || e.key !== " ")) {
      consume();
      setQuery((q) => q + e.key);
    }
  };

  return (
    <ContextMenuRoot onOpenChange={(open) => !open && setMenuNode(null)}>
      <ContextMenuTrigger asChild>
        <div ref={container} className="relative min-h-0 flex-1" onKeyDownCapture={onKeyDownCapture}>
          {query && (
            <div className="absolute top-1.5 right-2 z-10 flex items-center gap-1.5 rounded-md border border-accent bg-elevated px-2 py-0.5 text-[12px] text-fg shadow-popover">
              <Search className="size-3 text-subtle" />
              {query}
            </div>
          )}
          {width > 0 && height > 0 && (
            <Tree<TreeNode>
              ref={tree}
              data={data}
              width={width}
              height={height}
              rowHeight={ROW_HEIGHT}
              indent={14}
              paddingTop={4}
              paddingBottom={8}
              disableDrag
              disableDrop
              disableEdit
              disableMultiSelection
              openByDefault={false}
              searchTerm={query}
              searchMatch={(node, term) =>
                node.data.kind !== "message" && node.data.name.toLowerCase().includes(term.toLowerCase())
              }
              onToggle={onToggle}
              onSelect={(nodes) => useExplorerSelection.setState({ selection: toSelection(nodes[0]?.data) })}
              aria-label="Database Explorer"
              className="outline-none"
            >
              {(props) => <Row {...props} query={query} onActivate={activate} onMenu={setMenuNode} />}
            </Tree>
          )}
        </div>
      </ContextMenuTrigger>
      {menuNode && <NodeMenu node={menuNode} />}
    </ContextMenuRoot>
  );
}

function Row({
  node,
  style,
  query,
  onActivate,
  onMenu,
}: NodeRendererProps<TreeNode> & {
  query: string;
  onActivate: (node: NodeApi<TreeNode>) => void;
  onMenu: (node: TreeNode) => void;
}) {
  const d = node.data;
  return (
    <div
      style={style}
      className={cx(
        "group flex h-full items-center gap-1.5 pr-2 text-[12.5px] whitespace-nowrap",
        node.isSelected ? "bg-accent-soft text-fg" : "text-fg hover:bg-hover",
        node.isFocused && node.isSelected && "[.dv-active-group_&]:bg-accent [.dv-active-group_&]:text-accent-fg",
      )}
      onDoubleClick={() => onActivate(node)}
      onContextMenu={() => {
        node.select();
        node.focus();
        onMenu(d);
      }}
    >
      <span
        className={cx("flex size-4 shrink-0 items-center justify-center text-subtle", node.isLeaf && "invisible")}
        onClick={(e) => {
          e.stopPropagation();
          node.toggle();
        }}
      >
        <ChevronRight className={cx("size-3.5 transition-transform", node.isOpen && "rotate-90")} />
      </span>
      <NodeIcon node={d} />
      <span className={cx("truncate", d.kind === "message" && (d.tone === "error" ? "text-danger" : "text-muted"))}>
        <Highlight text={d.name} query={query} />
      </span>
      {d.detail && <span className="truncate text-[11.5px] opacity-55">{d.detail}</span>}
      {d.kind === "dataSource" && d.color && (
        <span className="ml-auto h-3 w-1 shrink-0 rounded-full" style={{ background: d.color }} />
      )}
    </div>
  );
}

function NodeIcon({ node }: { node: TreeNode }) {
  switch (node.kind) {
    case "dataSource":
      return (
        <span className="relative flex">
          <EngineIcon engine={node.engine!} />
          {node.status && (
            <span className="absolute -right-0.5 -bottom-0.5 scale-75">
              <StatusDot tone={STATUS_TONE[node.status]} />
            </span>
          )}
        </span>
      );
    case "schema":
    case "group":
      return <Folder className="size-3.5 shrink-0 text-subtle" />;
    case "table":
      return node.objectKind === "view" || node.objectKind === "materializedView" ? (
        <Eye className="size-3.5 shrink-0 text-[#8e7cc3]" />
      ) : (
        <Table2 className="size-3.5 shrink-0 text-[#5b8def]" />
      );
    case "column":
      if (node.column?.primaryKey) return <KeyRound className="size-3.5 shrink-0 text-[#d4a72c]" />;
      if (node.isForeignKey) return <Link2 className="size-3.5 shrink-0 text-[#5b8def]" />;
      return <Columns3 className="size-3.5 shrink-0 text-subtle" />;
    case "message":
      return node.tone === "error" ? (
        <CircleAlert className="size-3.5 shrink-0 text-danger" />
      ) : (
        <LoaderCircle className="size-3.5 shrink-0 animate-spin text-subtle" />
      );
  }
}

function Highlight({ text, query }: { text: string; query: string }) {
  const at = query ? text.toLowerCase().indexOf(query.toLowerCase()) : -1;
  if (at < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, at)}
      <mark className="rounded-sm bg-warning/35 text-inherit">{text.slice(at, at + query.length)}</mark>
      {text.slice(at + query.length)}
    </>
  );
}

function NodeMenu({ node }: { node: TreeNode }) {
  const connected = useDataSources((s) => s.explorers[node.sourceId]?.status === "connected");
  return (
    <ContextMenuContent>
      {node.kind === "table" && (
        <>
          <CommandItem id="explorer.openData" />
          <CommandItem id="explorer.copyName" />
          <ContextMenuSeparator />
        </>
      )}
      {node.kind === "column" && (
        <>
          <CommandItem id="explorer.copyName" />
          <ContextMenuSeparator />
        </>
      )}
      <CommandItem id="console.new" />
      <CommandItem id="explorer.refresh" />
      {node.kind === "dataSource" && (
        <>
          <ContextMenuSeparator />
          {connected ? <CommandItem id="datasource.disconnect" /> : <CommandItem id="datasource.connect" />}
          <CommandItem id="datasource.edit" />
          <CommandItem id="datasource.delete" />
        </>
      )}
    </ContextMenuContent>
  );
}

function toSelection(node: TreeNode | undefined): ExplorerSelection | null {
  if (!node) return null;
  const { sourceId, schema, table } = node;
  switch (node.kind) {
    case "dataSource":
    case "message":
      return { kind: "dataSource", sourceId };
    case "schema":
    case "group":
      return { kind: "schema", sourceId, schema: schema! };
    case "table":
      return { kind: "table", sourceId, schema: schema!, table: table! };
    case "column":
      return { kind: "column", sourceId, schema: schema!, table: table!, column: node.name };
  }
}
