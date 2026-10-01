import * as Menu from "@radix-ui/react-context-menu";
import type { ReactNode } from "react";
import { executeCommand, isEnabled, useCommands } from "../commands/registry";
import { Kbd } from "./primitives";

export const ContextMenuRoot = Menu.Root;
export const ContextMenuTrigger = Menu.Trigger;

export function ContextMenuContent({ children }: { children: ReactNode }) {
  return (
    <Menu.Portal>
      <Menu.Content className="z-50 min-w-[200px] rounded-md border border-border-strong bg-elevated p-1 text-[12.5px] shadow-popover outline-none">
        {children}
      </Menu.Content>
    </Menu.Portal>
  );
}

const itemClass =
  "flex h-7 items-center justify-between gap-6 rounded px-2 text-fg outline-none data-[disabled]:opacity-40 data-[highlighted]:bg-accent data-[highlighted]:text-accent-fg [&[data-highlighted]_kbd]:border-transparent [&[data-highlighted]_kbd]:bg-white/20 [&[data-highlighted]_kbd]:text-accent-fg";

/** A menu item for an action that is not a registered command. */
export function ContextMenuItem({
  label,
  onSelect,
  disabled,
}: {
  label: string;
  onSelect: () => void;
  disabled?: boolean;
}) {
  return (
    <Menu.Item disabled={disabled} onSelect={onSelect} className={itemClass}>
      <span>{label}</span>
    </Menu.Item>
  );
}

/** A menu item bound to a registered command: same title, shortcut and enablement everywhere. */
export function CommandItem({ id, label }: { id: string; label?: string }) {
  const command = useCommands((s) => s.commands[id]);
  if (!command) return null;
  return (
    <Menu.Item
      disabled={!isEnabled(command)}
      onSelect={() => executeCommand(id)}
      className={itemClass}
    >
      <span>{label ?? command.title}</span>
      {command.keybinding && <Kbd binding={command.keybinding} />}
    </Menu.Item>
  );
}

export function ContextMenuSeparator() {
  return <Menu.Separator className="my-1 h-px bg-border" />;
}
