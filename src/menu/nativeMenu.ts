import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { keymapHeld } from "../commands/keymapHold";
import { executeCommand, useCommands } from "../commands/registry";
import { buildMenuModel } from "./menuModel";

/** Registration bursts (app start, a panel mounting) rebuild the menu once. */
const REBUILD_DELAY_MS = 150;

/**
 * A key with a menu accelerator reaches either the webview or the menu,
 * depending on who AppKit asks first. Should both ever fire for one press,
 * the menu's run is dropped if the keymap just ran the same command.
 */
const DOUBLE_FIRE_WINDOW_MS = 250;
const keyboardRuns = new Map<string, number>();

/** Called by the keymap for every command it runs. */
export function noteKeyboardRun(commandId: string) {
  keyboardRuns.set(commandId, performance.now());
}

/** Keeps the native menu bar in sync with the command registry and runs what it picks. */
export function useNativeMenu() {
  const commands = useCommands((s) => s.commands);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      invoke("menu_set", { menus: buildMenuModel(Object.values(commands)) }).catch((e) =>
        console.error("menu update failed", e),
      );
    }, REBUILD_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [commands]);

  useEffect(() => {
    const unlisten = listen<string>("menu-command", ({ payload: commandId }) => runMenuCommand(commandId));
    return () => void unlisten.then((stop) => stop());
  }, []);
}

/**
 * Runs what the menu picked, by click or accelerator. Nothing runs while a
 * dialog holds the keymap: AppKit knows nothing of the dialog and keeps the
 * menu bar live, so its clicks and accelerators stop here.
 */
export function runMenuCommand(commandId: string) {
  if (keymapHeld()) return;
  const ranAt = keyboardRuns.get(commandId);
  if (ranAt !== undefined && performance.now() - ranAt < DOUBLE_FIRE_WINDOW_MS) return;
  executeCommand(commandId);
}
