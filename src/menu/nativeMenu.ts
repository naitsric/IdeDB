import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
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
    const unlisten = listen<string>("menu-command", ({ payload: commandId }) => {
      const ranAt = keyboardRuns.get(commandId);
      if (ranAt !== undefined && performance.now() - ranAt < DOUBLE_FIRE_WINDOW_MS) return;
      executeCommand(commandId);
    });
    return () => void unlisten.then((stop) => stop());
  }, []);
}
