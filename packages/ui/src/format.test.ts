import { describe, expect, it } from "vitest";
import { formatBytes, formatEta, formatPercent, formatSpeed } from "./format";

describe("formatBytes", () => {
  it("formats bytes below 1024 as-is", () => {
    expect(formatBytes(512)).toBe("512 B");
  });
  it("formats kilobytes", () => {
    expect(formatBytes(2048)).toBe("2.00 KB");
  });
  it("formats megabytes", () => {
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.00 MB");
  });
  it("handles null/undefined", () => {
    expect(formatBytes(null)).toBe("—");
    expect(formatBytes(undefined)).toBe("—");
  });
});

describe("formatSpeed", () => {
  it("formats zero speed", () => {
    expect(formatSpeed(0)).toBe("0 B/s");
  });
  it("formats a positive speed", () => {
    expect(formatSpeed(1024)).toBe("1.00 KB/s");
  });
});

describe("formatPercent", () => {
  it("returns 0 when total is missing", () => {
    expect(formatPercent(100, null)).toBe(0);
  });
  it("computes a rounded percentage", () => {
    expect(formatPercent(50, 200)).toBe(25);
  });
  it("clamps to 100", () => {
    expect(formatPercent(300, 200)).toBe(100);
  });
});

describe("formatEta", () => {
  it("returns an em dash when total or speed is unknown", () => {
    expect(formatEta(50, null, 10)).toBe("—");
    expect(formatEta(50, 200, 0)).toBe("—");
  });
  it("formats seconds", () => {
    expect(formatEta(0, 100, 10)).toBe("10s");
  });
  it("formats minutes", () => {
    expect(formatEta(0, 12000, 10)).toBe("20m");
  });
});
