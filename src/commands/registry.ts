import { create } from "zustand";

/**
 * Where keyboard focus is, for keys that mean different things in different
 * places (⌘⏎ runs a statement in the editor but submits edits in the grid).
 * Marked in the DOM with `data-focus-context`.
 */
export type FocusContext = "grid";

/**
 * Every user-facing action is a registered command, like IntelliJ's
 * AnAction. Registering one makes it available to the keymap, Search
 * Everywhere and (later) menus with no extra wiring.
 */
export interface Command {
  /** Stable id, `<area>.<action>`, e.g. `console.execute`. Keymap overrides key on it. */
  id: string;
  title: string;
  /** Group shown in Search Everywhere, e.g. "Console". */
  category: string;
  /** tinykeys syntax, e.g. `$mod+Enter`, `$mod+Shift+KeyA`. */
  keybinding?: string;
  /**
   * Limits the keybinding to when focus is inside this context. Commands may
   * share a binding: see {@link pickCommand}. Search Everywhere and menus
   * ignore it; `enabled` alone decides there.
   */
  context?: FocusContext;
  /** Extra search terms. */
  keywords?: string[];
  /** Whether the command can run right now. Disabled commands stay visible but inert. */
  enabled?: () => boolean;
  run: () => void | Promise<void>;
}

interface CommandRegistry {
  commands: Record<string, Command>;
}

export const useCommands = create<CommandRegistry>(() => ({ commands: {} }));

/** Registers commands and returns a function that unregisters them. */
export function registerCommands(commands: Command[]): () => void {
  useCommands.setState((s) => ({
    commands: { ...s.commands, ...Object.fromEntries(commands.map((c) => [c.id, c])) },
  }));
  return () =>
    useCommands.setState((s) => {
      const next = { ...s.commands };
      for (const c of commands) if (next[c.id] === c) delete next[c.id];
      return { commands: next };
    });
}

export function isEnabled(command: Command): boolean {
  return command.enabled?.() ?? true;
}

export function focusContext(): FocusContext | undefined {
  const marked = document.activeElement?.closest("[data-focus-context]");
  return (marked?.getAttribute("data-focus-context") as FocusContext | null) ?? undefined;
}

/**
 * Which of the commands sharing a key runs: those bound to the focused
 * context first, then context-free ones, in registration order; the first
 * enabled one wins. A command bound to another context never runs.
 */
export function pickCommand(candidates: readonly Command[], context: FocusContext | undefined): Command | undefined {
  const contextual = candidates.filter((c) => c.context !== undefined && c.context === context);
  const global = candidates.filter((c) => c.context === undefined);
  return [...contextual, ...global].find(isEnabled);
}

export function executeCommand(id: string): void {
  const command = useCommands.getState().commands[id];
  if (!command || !isEnabled(command)) return;
  void Promise.resolve(command.run()).catch((e) => console.error(`command ${id} failed`, e));
}
