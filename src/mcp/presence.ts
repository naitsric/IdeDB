import { t, useLanguage } from "../i18n";
import type { McpClient } from "./api";

/**
 * Whether an MCP client is around, from when it was last seen: the
 * protocol keeps no connection open, and a client that calls tools is
 * marked seen at most every 30 s.
 */
export const ONLINE_WINDOW_MS = 2 * 60_000;

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const shortDate = () => new Intl.DateTimeFormat(useLanguage.getState().locale, { month: "short", day: "numeric" });
const longDate = () =>
  new Intl.DateTimeFormat(useLanguage.getState().locale, { month: "short", day: "numeric", year: "numeric" });

export function isOnline(lastSeenAt: string | null, now: number): boolean {
  if (!lastSeenAt) return false;
  const age = now - Date.parse(lastSeenAt);
  // A clock that went back a little still counts as now.
  return age < ONLINE_WINDOW_MS;
}

/** `Sep 28`, or `Sep 28, 2025` outside the current year. */
export function formatDay(iso: string, now: number): string {
  const date = new Date(iso);
  return (date.getFullYear() === new Date(now).getFullYear() ? shortDate() : longDate()).format(date);
}

/** `Online`, `Last seen 5 min ago`, `Last seen yesterday`, `Never connected`… */
export function presenceLabel(lastSeenAt: string | null, now: number): string {
  if (!lastSeenAt) return t("presence.never");
  if (isOnline(lastSeenAt, now)) return t("presence.online");
  return t("presence.lastSeen", { ago: ago(Date.parse(lastSeenAt), now) });
}

/** How long ago `at` was, in the coarsest unit that still says something. */
export function ago(at: number, now: number): string {
  const age = Math.max(0, now - at);
  if (age < MINUTE) return t("presence.justNow");
  if (age < HOUR) return t("presence.minutesAgo", { count: Math.floor(age / MINUTE) });
  if (age < DAY) return t("presence.hoursAgo", { count: Math.floor(age / HOUR) });
  if (age < 2 * DAY) return t("presence.yesterday");
  if (age < 7 * DAY) return t("presence.daysAgo", { count: Math.floor(age / DAY) });
  return formatDay(new Date(at).toISOString(), now);
}

/** The short form, for lists: `Online`, `5 min ago`, `Sep 11`, `Never`. */
export function presenceShort(lastSeenAt: string | null, now: number): string {
  if (!lastSeenAt) return t("presence.neverShort");
  return isOnline(lastSeenAt, now) ? t("presence.online") : ago(Date.parse(lastSeenAt), now);
}

/** Clients not revoked and seen recently. */
export function onlineCount(clients: readonly McpClient[], now: number): number {
  return clients.filter((c) => !c.revokedAt && isOnline(c.lastSeenAt, now)).length;
}

/** `claude-code 2.1.0`: what a client says it is. */
export function clientInfoLabel(name: string | null, version: string | null): string | null {
  if (!name) return null;
  return version ? `${name} ${version}` : name;
}

/** Time left as `m:ss`, rounded up so `0:00` means it is over. */
export function formatCountdown(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
