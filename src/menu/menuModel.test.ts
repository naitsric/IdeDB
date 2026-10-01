import { describe, expect, it } from "vitest";
import type { Command } from "../commands/registry";
import { buildMenuModel, menuAccelerator, toAccelerator, type MenuEntry, type MenuSpec } from "./menuModel";

const command = (id: string, category: string, extra: Partial<Command> = {}): Command => ({
  id,
  title: id,
  category,
  run: () => {},
  ...extra,
});

const menu = (menus: MenuSpec[], title: string) => menus.find((m) => m.title === title);
const commandIds = (spec: MenuSpec | undefined) =>
  spec?.items.flatMap((i: MenuEntry) => (i.kind === "command" ? [i.commandId] : [])) ?? [];

describe("toAccelerator", () => {
  it.each([
    ["$mod+KeyO", "CmdOrCtrl+O"],
    ["$mod+Shift+KeyA", "CmdOrCtrl+Shift+A"],
    ["$mod+Alt+KeyY", "CmdOrCtrl+Alt+Y"],
    ["$mod+Alt+Shift+KeyC", "CmdOrCtrl+Alt+Shift+C"],
    ["$mod+Digit1", "CmdOrCtrl+1"],
    ["$mod+Semicolon", "CmdOrCtrl+Semicolon"],
    ["$mod+Enter", "CmdOrCtrl+Enter"],
    ["$mod+F2", "CmdOrCtrl+F2"],
    ["F4", "F4"],
    ["Control+KeyG", "Ctrl+G"],
    ["Meta+ArrowUp", "Cmd+ArrowUp"],
  ])("%s → %s", (binding, accelerator) => {
    expect(toAccelerator(binding)).toBe(accelerator);
  });

  it("rejects sequences, unknown modifiers and odd keys", () => {
    expect(toAccelerator("$mod+KeyK $mod+KeyC")).toBeNull();
    expect(toAccelerator("Hyper+KeyA")).toBeNull();
    expect(toAccelerator("$mod+a")).toBeNull();
  });
});

describe("menuAccelerator", () => {
  it("accelerates only unambiguous, global, non-reserved bindings", () => {
    const all = [
      command("navigate.table", "Navigate", { keybinding: "$mod+KeyO" }),
      command("console.execute", "Console", { keybinding: "$mod+Enter" }),
      command("grid.submit", "Data Editor", { keybinding: "$mod+Enter", context: "grid" }),
      command("editor.gotoDeclaration", "Navigate", { keybinding: "$mod+KeyB", context: "editor" }),
      command("grid.copy", "Results", { keybinding: "$mod+KeyC", context: "grid" }),
      command("search.find", "Navigate", { keybinding: "$mod+KeyF" }),
      command("view.resetLayout", "View"),
    ];
    const byId = (id: string) => menuAccelerator(all.find((c) => c.id === id)!, all);

    expect(byId("navigate.table")).toBe("CmdOrCtrl+O");
    expect(byId("console.execute")).toBeNull(); // shared with the grid's Submit
    expect(byId("grid.submit")).toBeNull(); // context-bound
    expect(byId("editor.gotoDeclaration")).toBeNull();
    expect(byId("grid.copy")).toBeNull();
    expect(byId("search.find")).toBeNull(); // ⌘F belongs to the editor
    expect(byId("view.resetLayout")).toBeNull();
  });
});

describe("buildMenuModel", () => {
  const commands = [
    command("datasource.new", "Data Source", { keybinding: "$mod+KeyN" }),
    command("grid.addRow", "Data Editor", { keybinding: "$mod+KeyN", context: "grid" }),
    command("console.new", "Console", { keybinding: "$mod+Shift+KeyL" }),
    command("console.execute", "Console", { keybinding: "$mod+Enter" }),
    command("view.toolWindow.explorer", "View", { keybinding: "$mod+Digit1" }),
    command("appearance.theme.dark", "Appearance"),
    command("explorer.refresh", "Database Explorer"),
    command("brand.new", "Something New"),
  ];
  const menus = buildMenuModel(commands);

  it("orders the bar like a macOS IDE", () => {
    expect(menus.map((m) => m.title)).toEqual(["IdeDB", "File", "Edit", "View", "Query", "Tools", "Window", "Help"]);
  });

  it("puts every registered command somewhere, unknown categories in Tools", () => {
    const placed = menus.flatMap(commandIds).sort();
    expect(placed).toEqual(commands.map((c) => c.id).sort());
    expect(commandIds(menu(menus, "Tools"))).toEqual(["brand.new"]);
  });

  it("keeps the standard Edit items first so text fields keep ⌘C/⌘V", () => {
    const edit = menu(menus, "Edit")!.items;
    expect(edit.slice(0, 7)).toEqual([
      { kind: "predefined", name: "undo" },
      { kind: "predefined", name: "redo" },
      { kind: "separator" },
      { kind: "predefined", name: "cut" },
      { kind: "predefined", name: "copy" },
      { kind: "predefined", name: "paste" },
      { kind: "predefined", name: "selectAll" },
    ]);
    expect(commandIds(menu(menus, "Edit"))).toEqual(["grid.addRow"]);
  });

  it("groups categories into sections in table order", () => {
    const view = menu(menus, "View")!.items;
    expect(view.map((i) => (i.kind === "command" ? i.commandId : i.kind))).toEqual([
      "view.toolWindow.explorer",
      "separator",
      "explorer.refresh",
      "separator",
      "appearance.theme.dark",
    ]);
  });

  it("drops the accelerator of shared keys but keeps unique ones", () => {
    const file = menu(menus, "File")!.items.find((i) => i.kind === "command" && i.commandId === "datasource.new");
    expect(file).toMatchObject({ accelerator: null });
    const query = menu(menus, "Query")!.items;
    expect(query.find((i) => i.kind === "command" && i.commandId === "console.new")).toMatchObject({
      accelerator: "CmdOrCtrl+Shift+L",
    });
  });

  it("replaces the predefined Quit with IdeDB's own, which asks about open transactions", () => {
    expect(menu(menus, "IdeDB")!.items.at(-1)).toEqual({ kind: "predefined", name: "quit" });

    const withQuit = buildMenuModel([
      ...commands,
      command("app.quit", "Application", { keybinding: "$mod+KeyQ" }),
      command("transaction.commit", "Transaction", { keybinding: "$mod+Alt+Enter" }),
    ]);
    const app = menu(withQuit, "IdeDB")!.items;
    expect(app.at(-1)).toEqual({ kind: "command", commandId: "app.quit", title: "app.quit", accelerator: "CmdOrCtrl+Q" });
    expect(app.some((i) => i.kind === "predefined" && i.name === "quit")).toBe(false);
    expect(commandIds(menu(withQuit, "Query"))).toContain("transaction.commit");
  });

  it("puts the MCP tool window in View and the server's commands first in Tools", () => {
    const withMcp = buildMenuModel([
      ...commands,
      command("view.toolWindow.mcp", "View", { keybinding: "$mod+Digit8" }),
      command("mcp.toggleServer", "MCP"),
      command("mcp.newClient", "MCP"),
      command("mcp.showActivity", "MCP"),
      command("mcp.showApprovals", "MCP"),
    ]);
    const view = menu(withMcp, "View")!.items;
    expect(view.slice(0, 2)).toEqual([
      { kind: "command", commandId: "view.toolWindow.explorer", title: "view.toolWindow.explorer", accelerator: "CmdOrCtrl+1" },
      { kind: "command", commandId: "view.toolWindow.mcp", title: "view.toolWindow.mcp", accelerator: "CmdOrCtrl+8" },
    ]);
    const tools = menu(withMcp, "Tools")!.items;
    expect(tools.map((i) => (i.kind === "command" ? i.commandId : i.kind))).toEqual([
      "mcp.toggleServer",
      "mcp.newClient",
      "mcp.showActivity",
      "mcp.showApprovals",
      "separator",
      "brand.new",
    ]);
  });

  it("ends with Window and a Help link to the repository", () => {
    expect(menu(menus, "Window")!.items.map((i) => (i.kind === "predefined" ? i.name : i.kind))).toEqual([
      "minimize",
      "zoom",
      "fullscreen",
    ]);
    expect(menu(menus, "Help")!.items).toEqual([
      { kind: "link", title: "IdeDB on GitHub", url: "https://github.com/naitsric/IdeDB" },
    ]);
  });
});
