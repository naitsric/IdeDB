import { useEffect } from "react";
import { tinykeys } from "tinykeys";
import { noteKeyboardRun } from "../menu/nativeMenu";
import { keymapHeld } from "./keymapHold";
import { executeCommand, focusContext, pickCommand, useCommands, type Command, type FocusContext } from "./registry";

/** Max gap between the two Shift taps of "Shift Shift", as in IntelliJ. */
const DOUBLE_SHIFT_MS = 350;

/**
 * What a key bound to `candidates` does: run one of them, do nothing at
 * all, or go to whoever else handles it.
 *
 * - The command `pickCommand` chooses runs and consumes the key. Without
 *   an enabled one, the key passes untouched.
 * - While a dialog holds the keymap (see keymapHold.ts) nothing runs. A
 *   key bound where focus is (globally, or to its context) is suppressed,
 *   enabled or not: its default action is prevented, so ⌘⏎ can't press
 *   the dialog's focused button either, though the dialog's handlers still
 *   see it. Keys bound only to other contexts pass, so ⌘C still copies
 *   text in the dialog.
 */
export function resolveKey(
  candidates: readonly Command[],
  context: FocusContext | undefined,
  held: boolean,
): { action: "run"; command: Command } | { action: "suppress" } | { action: "pass" } {
  if (held) {
    const bound = candidates.some((c) => c.context === undefined || c.context === context);
    return bound ? { action: "suppress" } : { action: "pass" };
  }
  const command = pickCommand(candidates, context);
  return command ? { action: "run", command } : { action: "pass" };
}

/**
 * Binds every registered command's keybinding on the window, in the capture
 * phase so it runs before editors and grids. Rebinds when the registry
 * changes. Unlike tinykeys' default, shortcuts also fire from inputs and
 * editors: all our bindings use a modifier or a function key, so they never
 * collide with typing.
 *
 * Several commands may share a key (see `pickCommand`). The one that runs
 * consumes the key (no default action, no propagation), so e.g. ⌘⏎ never
 * also inserts a newline in the editor; when none is enabled, the key goes
 * to whoever else handles it. Nothing runs while a dialog holds the keymap.
 */
export function useKeymap(onDoubleShift: () => void) {
  const commands = useCommands((s) => s.commands);

  useEffect(() => {
    const byBinding = new Map<string, Command[]>();
    for (const c of Object.values(commands)) {
      if (c.keybinding) byBinding.set(c.keybinding, [...(byBinding.get(c.keybinding) ?? []), c]);
    }
    const bindings = Object.fromEntries(
      [...byBinding].map(([binding, candidates]) => [
        binding,
        (event: KeyboardEvent) => {
          const resolved = resolveKey(candidates, focusContext(), keymapHeld());
          if (resolved.action === "pass") return;
          event.preventDefault();
          if (resolved.action === "suppress") return;
          event.stopPropagation();
          noteKeyboardRun(resolved.command.id);
          executeCommand(resolved.command.id);
        },
      ]),
    );
    return tinykeys(window, bindings, {
      capture: true,
      ignore: (event) => event.repeat || event.isComposing,
    });
  }, [commands]);

  useEffect(() => detectDoubleShift(() => !keymapHeld() && onDoubleShift()), [onDoubleShift]);
}

/** Fires on two bare Shift taps in quick succession, with no other key in between. */
function detectDoubleShift(onDoubleShift: () => void): () => void {
  let lastTap = 0;
  let clean = false;

  const down = (e: KeyboardEvent) => {
    if (e.key === "Shift" && !e.repeat) {
      clean = !(e.metaKey || e.ctrlKey || e.altKey);
    } else {
      clean = false;
      lastTap = 0;
    }
  };
  const up = (e: KeyboardEvent) => {
    if (e.key !== "Shift" || !clean) return;
    const now = performance.now();
    if (now - lastTap < DOUBLE_SHIFT_MS) {
      lastTap = 0;
      onDoubleShift();
    } else {
      lastTap = now;
    }
  };

  window.addEventListener("keydown", down, true);
  window.addEventListener("keyup", up, true);
  return () => {
    window.removeEventListener("keydown", down, true);
    window.removeEventListener("keyup", up, true);
  };
}

const SYMBOLS: Record<string, string> = {
  $mod: "⌘",
  Meta: "⌘",
  Control: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Enter: "⏎",
  Escape: "⎋",
  Backspace: "⌫",
  Delete: "⌦",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Tab: "⇥",
  Space: "Space",
};

/** `$mod+Shift+KeyA` → `⇧⌘A`, in macOS modifier order. */
export function formatKeybinding(binding: string): string {
  const order = ["Control", "Alt", "Shift", "$mod", "Meta"];
  const parts = binding.split("+");
  const key = parts.pop()!;
  const modifiers = parts.sort((a, b) => order.indexOf(a) - order.indexOf(b));
  const keyLabel = SYMBOLS[key] ?? key.replace(/^Key/, "").replace(/^Digit/, "");
  return modifiers.map((m) => SYMBOLS[m] ?? m).join("") + keyLabel;
}
