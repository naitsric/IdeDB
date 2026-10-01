import { describe, expect, it } from "vitest";
import type { McpClient } from "./api";
import {
  ago,
  clientInfoLabel,
  formatCountdown,
  isOnline,
  onlineCount,
  presenceLabel,
  presenceShort,
} from "./presence";

const NOW = Date.parse("2026-10-01T12:00:00.000Z");
const before = (ms: number) => new Date(NOW - ms).toISOString();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

const client = (id: string, lastSeenAt: string | null, revokedAt: string | null = null): McpClient => ({
  id,
  name: id,
  tokenPrefix: "idedb_abcdef",
  createdAt: "2026-09-01T12:00:00.000Z",
  lastSeenAt,
  lastClientName: null,
  lastClientVersion: null,
  revokedAt,
  grants: [],
});

describe("presence", () => {
  it("is online when seen in the last two minutes", () => {
    expect(isOnline(before(0), NOW)).toBe(true);
    expect(isOnline(before(2 * MIN - 1), NOW)).toBe(true);
    expect(isOnline(before(2 * MIN), NOW)).toBe(false);
    expect(isOnline(null, NOW)).toBe(false);
    // A timestamp slightly ahead of this clock is still now.
    expect(isOnline(new Date(NOW + 5_000).toISOString(), NOW)).toBe(true);
  });

  it("says when a client was last seen", () => {
    expect(presenceLabel(null, NOW)).toBe("Never connected");
    expect(presenceLabel(before(30_000), NOW)).toBe("Online");
    expect(presenceLabel(before(5 * MIN), NOW)).toBe("Last seen 5 min ago");
    expect(presenceLabel(before(59 * MIN), NOW)).toBe("Last seen 59 min ago");
    expect(presenceLabel(before(3 * HOUR + 20 * MIN), NOW)).toBe("Last seen 3 h ago");
    expect(presenceLabel(before(30 * HOUR), NOW)).toBe("Last seen yesterday");
    expect(presenceLabel(before(4 * DAY), NOW)).toBe("Last seen 4 days ago");
    expect(presenceLabel(before(20 * DAY), NOW)).toBe("Last seen Sep 11");
    expect(presenceLabel("2025-06-15T12:00:00.000Z", NOW)).toBe("Last seen Jun 15, 2025");
  });

  it("has a short form for lists", () => {
    expect(presenceShort(null, NOW)).toBe("Never");
    expect(presenceShort(before(30_000), NOW)).toBe("Online");
    expect(presenceShort(before(5 * MIN), NOW)).toBe("5 min ago");
    expect(presenceShort(before(20 * DAY), NOW)).toBe("Sep 11");
  });

  it("reads under a minute as just now", () => {
    expect(ago(NOW - 10_000, NOW)).toBe("just now");
    expect(ago(NOW + 10_000, NOW)).toBe("just now");
  });

  it("counts clients online, never revoked ones", () => {
    const clients = [
      client("a", before(10_000)),
      client("b", before(MIN)),
      client("c", before(10 * MIN)),
      client("d", null),
      client("e", before(5_000), before(1_000)),
    ];
    expect(onlineCount(clients, NOW)).toBe(2);
    expect(onlineCount([], NOW)).toBe(0);
  });
});

describe("clientInfoLabel", () => {
  it("joins name and version", () => {
    expect(clientInfoLabel("claude-code", "2.1.0")).toBe("claude-code 2.1.0");
    expect(clientInfoLabel("cursor", null)).toBe("cursor");
    expect(clientInfoLabel(null, "1.0")).toBeNull();
  });
});

describe("formatCountdown", () => {
  it("shows minutes and seconds, rounded up", () => {
    expect(formatCountdown(120_000)).toBe("2:00");
    expect(formatCountdown(105_000)).toBe("1:45");
    expect(formatCountdown(9_001)).toBe("0:10");
    expect(formatCountdown(1)).toBe("0:01");
    expect(formatCountdown(0)).toBe("0:00");
    expect(formatCountdown(-5_000)).toBe("0:00");
    expect(formatCountdown(3_600_000)).toBe("60:00");
  });
});
