import { describe, expect, it } from "vitest";
import { cx } from "./primitives";
import { inputClass } from "./Modal";

describe("cx", () => {
  it("lets a later width override the field's full width", () => {
    const port = cx(inputClass, "w-20 shrink-0").split(" ");
    expect(port).toContain("w-20");
    expect(port).not.toContain("w-full");
  });

  it("lets an error border replace the default border color", () => {
    const field = cx(inputClass, "border-danger focus:border-danger").split(" ");
    expect(field).toEqual(expect.arrayContaining(["border", "border-danger", "focus:border-danger"]));
    expect(field).not.toContain("border-border");
    expect(field).not.toContain("focus:border-accent");
  });

  it("keeps a text color and a text size apart, and replaces the size", () => {
    const mono = cx(inputClass, "font-mono text-[12px]").split(" ");
    expect(mono).toEqual(expect.arrayContaining(["text-fg", "text-[12px]", "placeholder:text-subtle"]));
    expect(mono).not.toContain("text-[12.5px]");
  });

  it("drops falsy entries", () => {
    expect(cx("a", false, undefined, null, "b")).toBe("a b");
  });
});
