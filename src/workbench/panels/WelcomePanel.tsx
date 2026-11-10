import { commandTitle, useCommands } from "../../commands/registry";
import { useT } from "../../i18n";
import { Kbd } from "../../ui/primitives";

const HINTS = [
  null,
  "navigate.table",
  "datasource.new",
  "console.new",
  "view.toolWindow.explorer",
] as const;

/** Shown in the editor area while no console is open, like an IDE's empty editor. */
export function WelcomePanel() {
  const t = useT();
  const commands = useCommands((s) => s.commands);
  return (
    <div className="flex h-full items-center justify-center bg-panel">
      <dl className="grid grid-cols-[auto_auto] items-center gap-x-6 gap-y-3 text-[13px]">
        {HINTS.map((id) => {
          const binding = id ? commands[id]?.keybinding : undefined;
          const label = id ? (commands[id] ? commandTitle(commands[id]) : "") : t("welcome.searchEverywhere");
          return (
            <div key={id ?? "search"} className="contents">
              <dt className="text-right text-muted">{label}</dt>
              <dd>{id ? binding && <Kbd binding={binding} /> : <Kbd>⇧⇧</Kbd>}</dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}
