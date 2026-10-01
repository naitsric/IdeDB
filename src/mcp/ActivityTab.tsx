import { Activity, ArrowUp, CircleAlert, LoaderCircle, Plus, Power, RefreshCw, Search, SearchX, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import type { DataSource } from "../db/api";
import { useDataSources } from "../db/dataSources";
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRoot,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "../ui/ContextMenu";
import { formatDuration } from "../ui/format";
import { Button, cx, IconButton, Kbd } from "../ui/primitives";
import { useElementSize } from "../ui/useElementSize";
import { useNow } from "../ui/useNow";
import {
  callSummary,
  canLoadOlder,
  consoleBlocker,
  countLabel,
  formatFullTime,
  formatLogTime,
  isFiltered,
  newerThan,
  oneLine,
  revealRow,
  stepSelection,
  visibleRange,
  type ActivityLog,
} from "./activity";
import { ActivityDetail, DecisionBadge, openInConsole, SourceDot } from "./ActivityDetail";
import type { AuditEntry, Decision } from "./api";
import { DECISION_LABEL } from "./labels";
import {
  clearActivityFilter,
  loadOlderActivity,
  openNewClient,
  reloadActivity,
  selectActivity,
  setActivityFilter,
  setMcpTab,
  startActivity,
  toggleServer,
  useMcp,
} from "./store";
import { EmptyState, LoadErrorBanner, ServerBanner, useServerOff } from "./TabParts";

/** Every row is this tall, so which rows are on screen is arithmetic. */
const ROW_HEIGHT = 44;
/** Below the rows: Load Older, loading, or the start of the log. */
const FOOTER_HEIGHT = 44;
/** Scrolled less than this counts as at the top, where new rows show as they come. */
const TOP_SLACK = ROW_HEIGHT / 2;
/** The next page loads when the end of the list is this close. */
const PREFETCH_ROWS = 20;
/** The search applies once typing pauses this long. */
const SEARCH_DELAY_MS = 200;
/** The list's height until it is measured. */
const FALLBACK_VIEWPORT = 600;

/**
 * The MCP Activity tab: every tool call clients made through IdeDB, newest
 * first and live, with filters on the left of the toolbar and the selected
 * call's details at the right.
 *
 * The log is long (the store keeps 20,000 rows), so only the rows in view
 * are rendered: rows have a fixed height, which makes that a matter of
 * arithmetic (see `visibleRange`), and lets new rows arriving at the top
 * keep the rows the user is reading in place.
 */
export function ActivityTab() {
  const loaded = useMcp((s) => s.loaded);
  const log = useMcp((s) => s.activity.log);
  const filter = useMcp((s) => s.activity.filter);
  const selectedId = useMcp((s) => s.activity.selectedId);
  const version = useMcp((s) => s.activity.version);
  const serverOff = useServerOff();
  const searchInput = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(startActivity, []);

  const selected = selectedId === null ? undefined : log.entries.find((e) => e.id === selectedId);
  const filtered = isFiltered(filter);
  // Nothing recorded at all (as opposed to nothing matching), or not known yet.
  const blank = !filtered && log.entries.length === 0 && !log.error;
  const settled = loaded && log.started && !log.loading;

  // ⌘F anywhere in the tab searches the SQL.
  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "f") {
      e.preventDefault();
      searchInput.current?.focus();
      searchInput.current?.select();
    }
  };

  if (blank) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <LoadErrorBanner />
        {/* The empty state says it when the server is off. */}
        {!serverOff && <ServerBanner />}
        {settled ? (
          <NothingYet serverOff={serverOff} />
        ) : (
          <div className="flex flex-1 items-center justify-center">
            <LoaderCircle className="size-4 animate-spin text-subtle" />
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" onKeyDown={onKeyDown}>
      <LoadErrorBanner />
      <ServerBanner />
      <Toolbar inputRef={searchInput} onLeave={() => list.current?.focus()} />
      <Group orientation="horizontal" className="min-h-0 flex-1">
        <Panel id="log" minSize="30%">
          <ActivityList
            // A log started over (a new filter, Refresh) is a new list: from the top, nothing counted as new.
            key={version}
            listRef={list}
            log={log}
            filtered={filtered}
            selectedId={selectedId}
          />
        </Panel>
        <Separator className="w-px bg-border transition-colors data-[separator=active]:bg-accent data-[separator=hover]:bg-accent" />
        <Panel id="detail" defaultSize="40%" minSize="20%">
          <ActivityDetail entry={selected} />
        </Panel>
      </Group>
    </div>
  );
}

/** The empty tab before any client called anything: how to get there. */
function NothingYet({ serverOff }: { serverOff: boolean }) {
  const noClients = useMcp((s) => s.clients.length === 0);
  if (serverOff) {
    return (
      <EmptyState icon={Power} title="The MCP server is off">
        Turn it on, then connect a client from the Clients tab. Every call it makes through IdeDB shows up here as it
        happens.
        <div className="mt-3 flex gap-2">
          <Button variant="primary" onClick={() => void toggleServer()}>
            Turn On
          </Button>
          <Button onClick={() => setMcpTab("clients")}>Go to Clients</Button>
        </div>
      </EmptyState>
    );
  }
  return (
    <EmptyState icon={Activity} title="No activity yet">
      Every call an MCP client makes through IdeDB shows up here as it happens: the SQL, who sent it, on which data
      source, and whether it ran.{" "}
      {noClients
        ? "Create a client, grant it access to a data source, and paste its configuration into the app, like Claude Code or Cursor."
        : "Paste a client's configuration from the Clients tab into the app, then ask it about your data."}
      <div className="mt-3 flex gap-2">
        {noClients && (
          <Button variant="primary" onClick={openNewClient}>
            <Plus className="size-3.5" /> New Client
          </Button>
        )}
        <Button onClick={() => setMcpTab("clients")}>Go to Clients</Button>
      </div>
    </EmptyState>
  );
}

// Toolbar

/** Search over the SQL, the three filters, and the count. */
function Toolbar({ inputRef, onLeave }: { inputRef: RefObject<HTMLInputElement | null>; onLeave: () => void }) {
  const filter = useMcp((s) => s.activity.filter);
  const log = useMcp((s) => s.activity.log);
  const clients = useMcp((s) => s.clients);
  const sources = useDataSources((s) => s.sources);
  const count = log.entries.length;

  const clientOptions = options(
    [...clients].sort(byName).map((c) => ({ value: c.id, label: c.name })),
    filter.clientId,
    () => log.entries.find((e) => e.clientId === filter.clientId)?.clientName ?? "Deleted client",
  );
  const sourceOptions = options(
    [...sources].sort(byName).map((s) => ({ value: s.id, label: s.name })),
    filter.dataSourceId,
    () => log.entries.find((e) => e.dataSourceId === filter.dataSourceId)?.dataSourceName ?? "Deleted data source",
  );

  return (
    <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border px-1.5">
      <SearchField inputRef={inputRef} onLeave={onLeave} />
      <FilterSelect
        label="Client"
        all="All clients"
        value={filter.clientId}
        options={clientOptions}
        onChange={(clientId) => setActivityFilter({ clientId })}
      />
      <FilterSelect
        label="Data source"
        all="All data sources"
        value={filter.dataSourceId}
        options={sourceOptions}
        onChange={(dataSourceId) => setActivityFilter({ dataSourceId })}
      />
      <FilterSelect
        label="Decision"
        all="All decisions"
        value={filter.decision}
        options={(Object.keys(DECISION_LABEL) as Decision[]).map((d) => ({ value: d, label: DECISION_LABEL[d] }))}
        onChange={(decision) => setActivityFilter({ decision: decision as Decision | null })}
      />
      {isFiltered(filter) && (
        <IconButton label="Clear Filters" className="size-6" onClick={clearActivityFilter}>
          <X className="size-3.5" />
        </IconButton>
      )}
      <span
        className="ml-auto shrink-0 px-1.5 text-[11px] text-subtle tabular-nums"
        title={log.exhausted ? undefined : "Older entries load as you scroll down"}
      >
        {count.toLocaleString("en-US")}
        {!log.exhausted && count > 0 && "+"} {count === 1 && log.exhausted ? "entry" : "entries"}
      </span>
      <IconButton label="Refresh" className="size-6" onClick={() => void reloadActivity()}>
        <RefreshCw className="size-3.5" />
      </IconButton>
    </div>
  );
}

const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name);

/** The choices, plus the current one when it is gone (a deleted client), so the select still shows it. */
function options(list: Option[], current: string | null, missing: () => string): Option[] {
  if (current === null || list.some((o) => o.value === current)) return list;
  return [...list, { value: current, label: missing() }];
}

interface Option {
  value: string;
  label: string;
}

/** Searches the SQL as you type. ↓ and Esc (when empty) go to the list; Esc clears first. */
function SearchField({ inputRef, onLeave }: { inputRef: RefObject<HTMLInputElement | null>; onLeave: () => void }) {
  const search = useMcp((s) => s.activity.filter.search);
  const [text, setText] = useState(search);
  // The search last sent to the store, to tell its changes from ours.
  const applied = useRef(search);

  // Cleared or changed from elsewhere (Clear Filters, Show Activity).
  useEffect(() => {
    if (search === applied.current) return;
    applied.current = search;
    setText(search);
  }, [search]);

  const apply = useCallback((value: string) => {
    const next = value.trim();
    if (next === applied.current) return;
    applied.current = next;
    setActivityFilter({ search: next });
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => apply(text), SEARCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [text, apply]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      apply(text);
    } else if (e.key === "Escape" && text !== "") {
      e.preventDefault();
      setText("");
      apply("");
    } else if (e.key === "Escape" || e.key === "ArrowDown") {
      e.preventDefault();
      onLeave();
    }
  };

  return (
    <label className="flex h-6 w-[clamp(9rem,28%,18rem)] min-w-0 shrink items-center gap-1.5 rounded-md border border-border bg-inset px-1.5 focus-within:border-accent">
      <Search className="size-3.5 shrink-0 text-subtle" />
      <input
        ref={inputRef}
        aria-label="Search SQL"
        placeholder="Search SQL"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        className="min-w-0 flex-1 bg-transparent font-mono text-[12px] text-fg outline-none placeholder:font-sans placeholder:text-subtle"
      />
      {text === "" && <Kbd binding="$mod+KeyF" />}
    </label>
  );
}

/** One filter, as a compact select; tinted while it narrows the log. */
function FilterSelect({
  label,
  all,
  value,
  options,
  onChange,
}: {
  label: string;
  all: string;
  value: string | null;
  options: Option[];
  onChange: (value: string | null) => void;
}) {
  return (
    <select
      aria-label={label}
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value === "" ? null : e.target.value)}
      className={cx(
        "h-6 max-w-40 min-w-0 shrink rounded-md border px-1 text-[12px] outline-none focus:border-accent",
        value === null
          ? "border-transparent bg-transparent text-muted hover:border-border"
          : "border-accent/40 bg-accent-soft text-fg",
      )}
    >
      <option value="">{all}</option>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

// The log

function ActivityList({
  listRef,
  log,
  filtered,
  selectedId,
}: {
  listRef: RefObject<HTMLDivElement | null>;
  log: ActivityLog;
  filtered: boolean;
  selectedId: number | null;
}) {
  const { entries } = log;
  const sources = useDataSources((s) => s.sources);
  const now = useNow(60_000);
  const size = useElementSize<HTMLDivElement>();
  const [scrollTop, setScrollTop] = useState(0);
  const viewport = size.height || FALLBACK_VIEWPORT;
  const firstId = entries[0]?.id ?? null;
  const atTop = scrollTop < TOP_SLACK;

  const setRefs = useCallback(
    (el: HTMLDivElement | null) => {
      listRef.current = el;
      return size.ref(el);
    },
    [listRef, size.ref],
  );

  // New rows come in at the top. Scrolled down, the rows being read stay
  // where they are, and the pill counts what came in above them.
  const shownFirst = useRef(firstId);
  useLayoutEffect(() => {
    const added = newerThan(entries, shownFirst.current);
    shownFirst.current = firstId;
    const el = listRef.current;
    if (added > 0 && el && el.scrollTop >= TOP_SLACK) {
      el.scrollTop += added * ROW_HEIGHT;
      setScrollTop(el.scrollTop);
    }
    // Only a new first row moves anything; older pages add at the bottom.
  }, [firstId]);

  // The newest row seen at the top; what came after it is new.
  const [seenId, setSeenId] = useState(firstId);
  useEffect(() => {
    if (atTop) setSeenId(firstId);
  }, [atTop, firstId]);
  const unseen = atTop ? 0 : newerThan(entries, seenId);

  const total = entries.length * ROW_HEIGHT + FOOTER_HEIGHT;
  const { start, end } = visibleRange(scrollTop, viewport, ROW_HEIGHT, entries.length);

  // Near the end, the next page loads by itself (not after a failure: Retry is there).
  useEffect(() => {
    const nearEnd = scrollTop + viewport >= entries.length * ROW_HEIGHT - PREFETCH_ROWS * ROW_HEIGHT;
    if (nearEnd && canLoadOlder(log) && !log.error && entries.length > 0) void loadOlderActivity();
  }, [scrollTop, viewport, entries.length, log]);

  const scrollTo = (top: number) => {
    const el = listRef.current;
    if (!el) return;
    el.scrollTop = top;
    setScrollTop(el.scrollTop);
  };

  const select = (id: number | null) => {
    selectActivity(id);
    const index = entries.findIndex((e) => e.id === id);
    if (index !== -1) scrollTo(revealRow(index, ROW_HEIGHT, listRef.current?.scrollTop ?? 0, viewport));
  };

  const selected = entries.find((e) => e.id === selectedId);
  const page = Math.max(1, Math.floor(viewport / ROW_HEIGHT) - 1);

  const onKeyDown = (e: KeyboardEvent) => {
    const step = (n: number) => {
      e.preventDefault();
      select(stepSelection(entries, selectedId, n));
    };
    if (e.key === "ArrowDown") step(1);
    else if (e.key === "ArrowUp") step(-1);
    else if (e.key === "PageDown") step(page);
    else if (e.key === "PageUp") step(-page);
    else if (e.key === "Home") step(-entries.length);
    else if (e.key === "End") step(entries.length);
    else if (e.key === "Enter" && selected) {
      e.preventDefault();
      openInConsole(selected);
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c" && selected?.sql) {
      e.preventDefault();
      void navigator.clipboard.writeText(selected.sql);
    }
  };

  let empty: ReactNode = null;
  if (entries.length === 0) {
    if (log.loading || !log.started) {
      empty = <LoaderCircle className="size-4 animate-spin text-subtle" />;
    } else if (!log.error && filtered) {
      empty = (
        <EmptyState icon={SearchX} title="No activity matches these filters">
          <Button className="mt-1" onClick={clearActivityFilter}>
            Clear Filters
          </Button>
        </EmptyState>
      );
    }
  }

  const sourceOf = (id: string | null) => (id === null ? undefined : sources.find((s) => s.id === id));
  const rendered = selectedId !== null && entries.slice(start, end).some((e) => e.id === selectedId);

  return (
    <div className="@container relative flex h-full min-w-0 flex-col">
      {unseen > 0 && (
        <button
          type="button"
          onClick={() => scrollTo(0)}
          className="absolute top-2 left-1/2 z-10 flex h-6 -translate-x-1/2 items-center gap-1 rounded-full bg-accent px-2.5 text-[11.5px] font-medium text-accent-fg shadow-popover hover:brightness-110"
        >
          <ArrowUp className="size-3" />
          {unseen.toLocaleString("en-US")} new
        </button>
      )}
      <div
        ref={setRefs}
        role="listbox"
        aria-label="MCP activity"
        tabIndex={0}
        aria-activedescendant={rendered ? `mcp-audit-${selectedId}` : undefined}
        onKeyDown={onKeyDown}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        className="min-h-0 flex-1 overflow-y-auto outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
      >
        {empty ? (
          <div className="flex h-full items-center justify-center">{empty}</div>
        ) : (
          <div className="relative" style={{ height: total }}>
            <div style={{ transform: `translateY(${start * ROW_HEIGHT}px)` }}>
              {entries.slice(start, end).map((entry) => (
                <ActivityRow
                  key={entry.id}
                  entry={entry}
                  source={sourceOf(entry.dataSourceId)}
                  selected={entry.id === selectedId}
                  now={now}
                />
              ))}
            </div>
            <div
              className="absolute inset-x-0 flex items-center justify-center text-[12px] text-subtle"
              style={{ top: entries.length * ROW_HEIGHT, height: FOOTER_HEIGHT }}
            >
              <LogFooter log={log} filtered={filtered} />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** Under the last row: what is older, if anything. */
function LogFooter({ log, filtered }: { log: ActivityLog; filtered: boolean }) {
  if (log.loading) {
    return (
      <span className="flex items-center gap-1.5">
        <LoaderCircle className="size-3.5 animate-spin" /> Loading older entries…
      </span>
    );
  }
  if (log.error) {
    return (
      <span className="flex min-w-0 items-center gap-2 px-3 text-danger">
        <CircleAlert className="size-3.5 shrink-0" />
        <span className="selectable truncate" title={log.error}>
          Couldn't read the log: {log.error}
        </span>
        <Button className="h-6 shrink-0" onClick={() => void loadOlderActivity()}>
          Retry
        </Button>
      </span>
    );
  }
  if (log.exhausted) return <span>{filtered ? "No older entries match" : "Start of the log"}</span>;
  return (
    <Button variant="ghost" className="h-6" onClick={() => void loadOlderActivity()}>
      Load Older
    </Button>
  );
}

/**
 * One tool call, on two lines: when, who, where and what kind, then its
 * SQL on one line. The decision leads, so refusals stand out.
 */
function ActivityRow({
  entry,
  source,
  selected,
  now,
}: {
  entry: AuditEntry;
  source: DataSource | undefined;
  selected: boolean;
  now: number;
}) {
  const counted = countLabel(entry);
  const blocker = consoleBlocker(entry, !!source);
  const kind = entry.statementKind ? `${entry.tool} · ${entry.statementKind}` : entry.tool;

  return (
    <ContextMenuRoot>
      <ContextMenuTrigger asChild>
        <div
          id={`mcp-audit-${entry.id}`}
          data-entry={entry.id}
          role="option"
          aria-selected={selected}
          onMouseDown={() => selectActivity(entry.id)}
          onContextMenu={() => selectActivity(entry.id)}
          onDoubleClick={() => openInConsole(entry)}
          style={{ height: ROW_HEIGHT }}
          className={cx(
            "flex items-center gap-2.5 border-b border-border/60 px-3",
            selected ? "bg-accent-soft" : "hover:bg-hover",
          )}
        >
          <DecisionBadge decision={entry.decision} className="w-[68px] justify-center" />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 items-center gap-1.5 text-[11.5px] leading-4">
              <time dateTime={entry.at} title={formatFullTime(entry.at)} className="shrink-0 text-subtle tabular-nums">
                {formatLogTime(entry.at, now)}
              </time>
              <span className="min-w-10 shrink truncate font-medium text-fg">{entry.clientName}</span>
              {entry.dataSourceName && (
                <span
                  className="flex min-w-10 shrink-[2] items-center gap-1 text-muted"
                  title={source ? undefined : `${entry.dataSourceName} no longer exists`}
                >
                  <SourceDot source={source} />
                  <span className="truncate">{entry.dataSourceName}</span>
                </span>
              )}
              {/* On a narrow log the kind goes first, then the duration: the detail has both. */}
              <span className="min-w-0 shrink-[4] truncate text-subtle @max-[34rem]:hidden">{kind}</span>
              <span className="ml-auto shrink-0 pl-2 whitespace-pre text-subtle tabular-nums">
                {counted}
                {entry.elapsedMs !== null && (
                  <span className="@max-[28rem]:hidden">
                    {counted && " · "}
                    {formatDuration(entry.elapsedMs)}
                  </span>
                )}
              </span>
            </div>
            <div className="mt-0.5 flex min-w-0 items-center gap-1.5 leading-4">
              {entry.error && (
                <span role="img" aria-label="Failed" title={entry.error} className="shrink-0 text-danger">
                  <CircleAlert className="size-3" />
                </span>
              )}
              {entry.sql !== null ? (
                <span className="truncate font-mono text-[12px] text-fg">{oneLine(entry.sql)}</span>
              ) : (
                <span className="truncate text-[12px] text-muted">{callSummary(entry)}</span>
              )}
            </div>
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem label="Open in Console" disabled={blocker !== null} onSelect={() => openInConsole(entry)} />
        <ContextMenuItem
          label="Copy SQL"
          disabled={entry.sql === null}
          onSelect={() => void navigator.clipboard.writeText(entry.sql ?? "")}
        />
        <ContextMenuSeparator />
        <ContextMenuItem
          label={`Only ${entry.clientName}`}
          disabled={entry.clientId === null}
          onSelect={() => setActivityFilter({ clientId: entry.clientId })}
        />
        <ContextMenuItem
          label={`Only ${entry.dataSourceName ?? "This Data Source"}`}
          disabled={entry.dataSourceId === null}
          onSelect={() => setActivityFilter({ dataSourceId: entry.dataSourceId })}
        />
        <ContextMenuItem
          label={`Only ${DECISION_LABEL[entry.decision]}`}
          onSelect={() => setActivityFilter({ decision: entry.decision })}
        />
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}
