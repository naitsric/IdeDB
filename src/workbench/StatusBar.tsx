import { useT } from "../i18n";
import { isRunning, useConsoles } from "../db/consoles";
import { useDataSources } from "../db/dataSources";
import { useTransactions } from "../db/transactions";
import { McpStatusItem } from "../mcp/McpStatusItem";
import { ENGINE_LABEL } from "../ui/EngineIcon";
import { cx, StatusDot } from "../ui/primitives";
import { useWorkbench } from "./bridge";

export function StatusBar() {
  const t = useT();
  const connected = useDataSources((s) => Object.values(s.explorers).filter((e) => e.status === "connected").length);
  const activeConsoleId = useWorkbench((s) => s.activeConsoleId);
  const dataSourceId = useConsoles((s) => (activeConsoleId ? s.consoles[activeConsoleId]?.dataSourceId : undefined));
  const running = useConsoles((s) => isRunning(activeConsoleId ? s.consoles[activeConsoleId] : undefined));
  const inTransaction = useConsoles((s) => !!(activeConsoleId && s.consoles[activeConsoleId]?.inTransaction));
  const mode = useTransactions((s) => (activeConsoleId ? (s.modes[activeConsoleId] ?? "auto") : undefined));
  const failed = useTransactions((s) => !!(activeConsoleId && s.open[activeConsoleId]?.failed));
  const server = useDataSources((s) => (dataSourceId ? s.explorers[dataSourceId]?.server : undefined));

  return (
    <footer className="flex h-[var(--statusbar-height)] shrink-0 items-center gap-3 border-t border-border bg-bg px-3 text-[11px] text-subtle">
      <span className="flex items-center gap-1.5">
        <StatusDot tone={connected > 0 ? "success" : "idle"} />
        {connected === 0 ? t("status.noConnections") : t("status.connected", { count: connected })}
      </span>
      {running && <span className="text-accent">{t("status.executing")}</span>}
      {inTransaction && (
        <span className={cx("font-medium", failed ? "text-danger" : "text-warning")}>
          {failed ? t("status.transactionFailed") : t("status.transactionOpen")}
        </span>
      )}
      <span className="ml-auto flex items-center gap-3">
        <McpStatusItem />
        {mode && (
          <span title={t("status.txModeHint")}>
            {t("status.tx", { mode: mode === "manual" ? t("tx.mode.manual") : t("tx.mode.auto") })}
          </span>
        )}
        {server && (
          <span>
            {ENGINE_LABEL[server.engine]} {server.version}
          </span>
        )}
      </span>
    </footer>
  );
}
