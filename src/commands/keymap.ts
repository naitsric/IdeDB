import { useEffect } from "react";
import { tinykeys } from "tinykeys";
import { noteKeyboardRun } from "../menu/nativeMenu";
import { executeCommand, focusContext, pickCommand, useCommands, type Command } from "./registry";

/** Max gap between the two Shift taps of "Shift Shift", as in IntelliJ. */
const DOUBLE_SHIFT_MS = 350;

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
 * to whoever else handles it.
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
          const command = pickCommand(candidates, focusContext());
          if (!command) return;
          event.preventDefault();
          event.stopPropagation();
          noteKeyboardRun(command.id);
          executeCommand(command.id);
        },
      ]),
    );
    return tinykeys(window, bindings, {
      capture: true,
      ignore: (event) => event.repeat || event.isComposing,
    });
  }, [commands]);

  useEffect(() => detectDoubleShift(onDoubleShift), [onDoubleShift]);
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
