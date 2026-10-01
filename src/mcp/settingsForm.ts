import type { McpSettings } from "./api";

/**
 * The Server tab's form: what the user typed, checked against the ranges
 * the server accepts (`McpSettings::validate` in crates/idedb-mcp).
 */

export type SettingsField = "port" | "maxRows" | "statementTimeoutSecs" | "writeTimeoutSecs" | "approvalTimeoutSecs";

export type SettingsDraft = Record<SettingsField, string>;

export const FIELD_RANGE: Record<SettingsField, readonly [number, number]> = {
  port: [1, 65535],
  maxRows: [1, 1000],
  statementTimeoutSecs: [1, 3600],
  writeTimeoutSecs: [1, 86400],
  approvalTimeoutSecs: [1, 3600],
};

const FIELDS = Object.keys(FIELD_RANGE) as SettingsField[];

const count = new Intl.NumberFormat("en-US");

export function draftOf(settings: McpSettings): SettingsDraft {
  return Object.fromEntries(FIELDS.map((field) => [field, String(settings[field])])) as SettingsDraft;
}

function rangeError(field: SettingsField): string {
  const [min, max] = FIELD_RANGE[field];
  return field === "port" ? `A port from ${min} to ${max}.` : `A whole number from ${min} to ${count.format(max)}.`;
}

/**
 * The settings the draft describes, on top of `base` (which keeps
 * `enabled`), or the error of each field that is out of range.
 */
export function parseDraft(
  draft: SettingsDraft,
  base: McpSettings,
): { settings: McpSettings | null; errors: Partial<Record<SettingsField, string>> } {
  const settings = { ...base };
  const errors: Partial<Record<SettingsField, string>> = {};
  for (const field of FIELDS) {
    const text = draft[field].trim();
    const value = Number(text);
    const [min, max] = FIELD_RANGE[field];
    if (!/^\d+$/.test(text) || value < min || value > max) errors[field] = rangeError(field);
    else settings[field] = value;
  }
  return { settings: Object.keys(errors).length ? null : settings, errors };
}

/** Whether the draft differs from what is saved. */
export function draftChanged(draft: SettingsDraft, saved: McpSettings): boolean {
  return FIELDS.some((field) => draft[field].trim() !== String(saved[field]));
}
