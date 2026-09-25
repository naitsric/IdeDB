import { useCommands } from "../../commands/registry";
import { Kbd } from "../../ui/primitives";

const HINTS = [
  ["Search Everywhere", null],
  ["Go to Table", "navigate.table"],
  ["New Data Source", "datasource.new"],
  ["New Query Console", "console.new"],
  ["Database Explorer", "view.toolWindow.explorer"],
] as const;

/** Shown in the editor area while no console is open, like an IDE's empty editor. */
export function WelcomePanel() {
  const commands = useCommands((s) => s.commands);
  return (
    <div className="flex h-full items-center justify-center bg-panel">
      <dl className="grid grid-cols-[auto_auto] items-center gap-x-6 gap-y-3 text-[13px]">
        {HINTS.map(([label, id]) => {
          const binding = id ? commands[id]?.keybinding : undefined;
          return (
            <div key={label} className="contents">
              <dt className="text-right text-muted">{label}</dt>
              <dd>{id ? binding && <Kbd binding={binding} /> : <Kbd>⇧⇧</Kbd>}</dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}
