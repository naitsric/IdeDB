import { describe, expect, it } from "vitest";
import { moreAfter, parsePageSize, rowsLabel, shouldFetchMore } from "./fetching";

describe("rowsLabel", () => {
  it("marks results with rows left", () => {
    expect(rowsLabel(500, "open")).toBe("500+ rows");
    expect(rowsLabel(1500, "closed")).toBe("1,500+ rows");
  });

  it("counts complete results exactly", () => {
    expect(rowsLabel(42, "none")).toBe("42 rows");
    expect(rowsLabel(1, "none")).toBe("1 row");
    expect(rowsLabel(0, "none")).toBe("0 rows");
  });
});

describe("shouldFetchMore", () => {
  it("fetches when the view nears the last loaded row", () => {
    expect(shouldFetchMore(420, 500, "open", false)).toBe(true);
    expect(shouldFetchMore(499, 500, "open", false)).toBe(true);
  });

  it("waits while the view is far from the end", () => {
    expect(shouldFetchMore(100, 500, "open", false)).toBe(false);
  });

  it("never fetches without an open rest or while busy", () => {
    expect(shouldFetchMore(499, 500, "none", false)).toBe(false);
    expect(shouldFetchMore(499, 500, "closed", false)).toBe(false);
    expect(shouldFetchMore(499, 500, "open", true)).toBe(false);
  });

  it("fetches right away when the first page fits in view", () => {
    expect(shouldFetchMore(3, 5, "open", false)).toBe(true);
  });
});

describe("moreAfter", () => {
  it("keeps an open rest open", () => {
    expect(moreAfter({ hasMore: true, cancelled: false })).toBe("open");
    expect(moreAfter({ hasMore: true, cancelled: true })).toBe("open");
  });

  it("tells a finished result from one a cancel cut short", () => {
    expect(moreAfter({ hasMore: false, cancelled: false })).toBe("none");
    expect(moreAfter({ hasMore: false, cancelled: true })).toBe("closed");
  });
});

describe("parsePageSize", () => {
  it("accepts whole numbers in range", () => {
    expect(parsePageSize("500")).toBe(500);
    expect(parsePageSize(" 1000 ")).toBe(1000);
  });

  it("rejects anything else", () => {
    for (const text of ["", "0", "-5", "1.5", "abc", "2000000"]) expect(parsePageSize(text)).toBeNull();
  });
});
