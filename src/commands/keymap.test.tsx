// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { runMenuCommand } from "../menu/nativeMenu";
import { resolveKey, useKeymap } from "./keymap";
import { holdKeymap, keymapHeld } from "./keymapHold";
import { registerCommands, type Command } from "./registry";

// React's act() wants to know it runs in a test.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const command = (id: string, extra: Partial<Command> = {}): Command => ({
  id,
  title: id,
  category: "Test",
  keybinding: "$mod+Enter",
  run: () => {},
  ...extra,
});

describe("resolveKey", () => {
  const execute = command("console.execute");
  const submit = command("grid.submit", { context: "grid" });
  const copy = command("grid.copy", { keybinding: "$mod+KeyC", context: "grid" });

  it("runs the command that applies", () => {
    expect(resolveKey([execute, submit], "grid", false)).toEqual({ action: "run", command: submit });
    expect(resolveKey([execute, submit], undefined, false)).toEqual({ action: "run", command: execute });
    expect(resolveKey([command("idle", { enabled: () => false })], undefined, false)).toEqual({ action: "pass" });
  });

  it("runs nothing while held, and suppresses the keys bound where focus is", () => {
    expect(resolveKey([execute, submit], undefined, true)).toEqual({ action: "suppress" });
    // Disabled, it would do nothing; held, it still mustn't press a focused button.
    expect(resolveKey([command("idle", { enabled: () => false })], undefined, true)).toEqual({ action: "suppress" });
    // Bound to the grid only: ⌘C still copies text in the dialog.
    expect(resolveKey([copy], undefined, true)).toEqual({ action: "pass" });
  });
});

describe("holding the keymap", () => {
  it("holds while any holder does", () => {
    const first = holdKeymap();
    const second = holdKeymap();
    first();
    expect(keymapHeld()).toBe(true);
    second();
    second();
    expect(keymapHeld()).toBe(false);
  });
});

describe("useKeymap", () => {
  const ran: string[] = [];
  const searched = vi.fn();
  let root: Root;
  let unregister: () => void;

  function Host() {
    useKeymap(searched);
    return <button type="button">focus</button>;
  }

  beforeEach(async () => {
    ran.length = 0;
    searched.mockClear();
    unregister = registerCommands([
      command("console.execute", { run: () => void ran.push("console.execute") }),
      command("view.toolWindow.mcp", { keybinding: "$mod+Digit8", run: () => void ran.push("view.toolWindow.mcp") }),
    ]);
    const host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    await act(async () => root.render(<Host />));
  });

  afterEach(async () => {
    unregister();
    await act(async () => root.unmount());
    document.body.innerHTML = "";
  });

  /** tinykeys reads `$mod` from the platform: ⌘ on macOS, Ctrl elsewhere (happy-dom says Linux). */
  const mod = /Mac|iPhone|iPad/.test(navigator.platform) ? { metaKey: true } : { ctrlKey: true };

  function press(key: string, code: string, modifiers: KeyboardEventInit = mod) {
    const event = new KeyboardEvent("keydown", { key, code, bubbles: true, cancelable: true, ...modifiers });
    document.querySelector("button")!.dispatchEvent(event);
    return event;
  }

  function tapShift() {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Shift", code: "ShiftLeft" }));
    window.dispatchEvent(new KeyboardEvent("keyup", { key: "Shift", code: "ShiftLeft" }));
  }

  it("runs a command on its key and consumes the key", () => {
    const event = press("Enter", "Enter");
    expect(ran).toEqual(["console.execute"]);
    expect(event.defaultPrevented).toBe(true);
  });

  it("runs no shortcut, menu command or Shift Shift while a dialog holds it", () => {
    const release = holdKeymap();
    const enter = press("Enter", "Enter");
    press("8", "Digit8");
    runMenuCommand("view.toolWindow.mcp");
    tapShift();
    tapShift();
    expect(ran).toEqual([]);
    expect(searched).not.toHaveBeenCalled();
    // Its default action is gone too, so it can't press the dialog's focused button.
    expect(enter.defaultPrevented).toBe(true);
    // Keys no command is bound to reach the dialog as they are.
    expect(press("Enter", "Enter", {}).defaultPrevented).toBe(false);
    expect(press("Tab", "Tab", {}).defaultPrevented).toBe(false);

    release();
    press("Enter", "Enter");
    runMenuCommand("view.toolWindow.mcp");
    tapShift();
    tapShift();
    expect(ran).toEqual(["console.execute", "view.toolWindow.mcp"]);
    expect(searched).toHaveBeenCalledOnce();
  });
});
