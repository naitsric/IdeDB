import { describe, expect, it } from "vitest";
import { passwordToSave } from "./dialogs";

describe("password on save", () => {
  it("stores an empty password for a new data source that saves it", () => {
    expect(passwordToSave(true, true, undefined)).toBe("");
    expect(passwordToSave(true, true, "s3cret")).toBe("s3cret");
  });

  it("keeps the stored password when editing without typing one", () => {
    expect(passwordToSave(false, true, undefined)).toBeUndefined();
    expect(passwordToSave(false, true, "")).toBe("");
  });

  it("never sends a password for a data source that does not save it", () => {
    expect(passwordToSave(true, false, "s3cret")).toBeUndefined();
  });
});
