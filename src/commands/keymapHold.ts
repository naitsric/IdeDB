import { useLayoutEffect } from "react";

/**
 * A dialog that must be answered before anything else, like the MCP
 * approval dialog, holds the keymap while it shows. Then no global
 * keybinding runs its command (⌘⏎ doesn't run the console behind it), the
 * native menu runs nothing, and Shift Shift opens nothing; the dialog's
 * own keys (Tab, Enter, Space, Escape) reach it as usual.
 *
 * Checked where keys are dispatched (see keymap.ts and nativeMenu.ts), not
 * by each command.
 */
const holds = new Set<symbol>();

/** Holds the keymap until the returned function is called. */
export function holdKeymap(): () => void {
  const hold = Symbol("keymap hold");
  holds.add(hold);
  return () => void holds.delete(hold);
}

export function keymapHeld(): boolean {
  return holds.size > 0;
}

/** Holds the keymap while the calling component is mounted. */
export function useKeymapHold() {
  // Before paint, so no key gets through between showing and holding.
  useLayoutEffect(() => holdKeymap(), []);
}
