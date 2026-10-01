import { describe, expect, it } from "vitest";
import type { McpSettings } from "./api";
import { draftChanged, draftOf, parseDraft } from "./settingsForm";

const saved: McpSettings = {
  enabled: true,
  port: 7412,
  maxRows: 200,
  statementTimeoutSecs: 30,
  writeTimeoutSecs: 600,
  approvalTimeoutSecs: 120,
};

describe("settings form", () => {
  it("round-trips the saved settings", () => {
    const draft = draftOf(saved);
    expect(draft.port).toBe("7412");
    expect(parseDraft(draft, saved)).toEqual({ settings: saved, errors: {} });
    expect(draftChanged(draft, saved)).toBe(false);
  });

  it("parses edits on top of the saved settings, keeping enabled", () => {
    const draft = { ...draftOf(saved), port: " 7500 ", writeTimeoutSecs: "86400" };
    expect(parseDraft(draft, saved).settings).toEqual({ ...saved, port: 7500, writeTimeoutSecs: 86400 });
    expect(draftChanged(draft, saved)).toBe(true);
  });

  it("flags each field out of the server's range", () => {
    const draft = {
      port: "70000",
      maxRows: "0",
      statementTimeoutSecs: "1.5",
      writeTimeoutSecs: "86401",
      approvalTimeoutSecs: "",
    };
    const { settings, errors } = parseDraft(draft, saved);
    expect(settings).toBeNull();
    expect(errors).toEqual({
      port: "A port from 1 to 65535.",
      maxRows: "A whole number from 1 to 1,000.",
      statementTimeoutSecs: "A whole number from 1 to 3,600.",
      writeTimeoutSecs: "A whole number from 1 to 86,400.",
      approvalTimeoutSecs: "A whole number from 1 to 3,600.",
    });
  });

  it("refuses signs, exponents and hex", () => {
    for (const port of ["-1", "+80", "1e3", "0x50"]) {
      expect(parseDraft({ ...draftOf(saved), port }, saved).errors.port).toBeDefined();
    }
  });
});
