import { describe, expect, it } from "vitest";
import { pickCommand, type Command } from "./registry";

const command = (id: string, extra: Partial<Command> = {}): Command => ({
  id,
  title: id,
  category: "Test",
  keybinding: "$mod+Enter",
  run: () => {},
  ...extra,
});

describe("shared keybindings", () => {
  const execute = command("console.execute");
  const submit = command("grid.submit", { context: "grid" });

  it("prefers the command bound to the focused context", () => {
    expect(pickCommand([execute, submit], "grid")?.id).toBe("grid.submit");
  });

  it("falls back to global commands when the contextual one is disabled or elsewhere", () => {
    const idle = command("grid.submit", { context: "grid", enabled: () => false });
    expect(pickCommand([execute, idle], "grid")?.id).toBe("console.execute");
    expect(pickCommand([execute, submit], undefined)?.id).toBe("console.execute");
  });

  it("runs nothing when every candidate is disabled", () => {
    expect(pickCommand([command("a", { enabled: () => false })], undefined)).toBeUndefined();
  });
});
