import { describe, expect, it } from "vitest";

import { formatBytes, formatDuration, formatPercent, formatRelativeTime } from "@/lib/format";

describe("formatDuration", () => {
  it.each([
    [0, "0 ms"],
    [3, "3 ms"],
    [999, "999 ms"],
    [1000, "1 s"],
    [1840, "1.8 s"],
    [12_400, "12 s"],
    [60_000, "1 min"],
    [125_000, "2 min 5 s"],
  ])("formats %d ms as %s", (ms, expected) => {
    expect(formatDuration(ms)).toBe(expected);
  });
});

describe("formatPercent", () => {
  it.each([
    [0, "0%"],
    [0.67, "67%"],
    [0.799, "79%"],
    [0.8, "80%"],
    [1, "100%"],
  ])("formats %d as %s", (ratio, expected) => {
    expect(formatPercent(ratio)).toBe(expected);
  });
});

const now = new Date("2026-11-16T12:00:00Z");

describe("formatBytes", () => {
  it.each([
    [1, "1 byte"],
    [820, "820 bytes"],
    [1024, "1 KB"],
    [1536, "1.5 KB"],
    [14_336, "14 KB"],
    [3_250_000, "3.1 MB"],
    [2 * 1024 ** 3, "2 GB"],
  ])("formats %d as %s", (size, expected) => {
    expect(formatBytes(size)).toBe(expected);
  });
});

describe("formatRelativeTime", () => {
  it.each([
    ["2026-11-16T11:59:30Z", "just now"],
    ["2026-11-16T12:00:20Z", "just now"],
    ["2026-11-16T11:59:00Z", "1 min ago"],
    ["2026-11-16T11:57:00Z", "3 min ago"],
    ["2026-11-16T11:00:01Z", "59 min ago"],
    ["2026-11-16T11:00:00Z", "1 h ago"],
    ["2026-11-15T12:00:01Z", "23 h ago"],
    ["2026-11-15T12:00:00Z", "yesterday"],
    ["2026-11-12T12:00:00Z", "4 days ago"],
    ["2026-11-09T12:00:00Z", "9 Nov"],
    ["2025-11-09T12:00:00Z", "9 Nov 2025"],
  ])("formats %s as %s", (iso, expected) => {
    expect(formatRelativeTime(iso, now)).toBe(expected);
  });

  it("returns an empty string for an invalid timestamp", () => {
    expect(formatRelativeTime("not a date", now)).toBe("");
  });
});
