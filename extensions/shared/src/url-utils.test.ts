import { describe, expect, it } from "vitest";
import {
  dedupeUrls,
  extensionOf,
  extractUrls,
  isAutoDetectableMedia,
  shouldIntercept,
} from "./url-utils";

describe("extractUrls", () => {
  it("finds a single bare URL", () => {
    expect(extractUrls("https://example.com/file.zip")).toEqual(["https://example.com/file.zip"]);
  });

  it("finds multiple URLs embedded in prose", () => {
    const text =
      "Check out https://example.com/a.mp4 and also http://cdn.example.org/b.pdf, both great.";
    expect(extractUrls(text)).toEqual([
      "https://example.com/a.mp4",
      "http://cdn.example.org/b.pdf",
    ]);
  });

  it("trims trailing punctuation glued on from prose", () => {
    expect(extractUrls("See https://example.com/file.zip.")).toEqual([
      "https://example.com/file.zip",
    ]);
    expect(extractUrls("(https://example.com/file.zip)")).toEqual(["https://example.com/file.zip"]);
  });

  it("returns an empty array when there are no URLs", () => {
    expect(extractUrls("no links here, just words")).toEqual([]);
  });

  it("handles a markdown link's href sitting right after the label", () => {
    const text = "[download](https://example.com/thing.iso) now";
    expect(extractUrls(text)).toEqual(["https://example.com/thing.iso"]);
  });
});

describe("dedupeUrls", () => {
  it("drops exact repeats", () => {
    const urls = ["https://a.com/x", "https://a.com/x", "https://b.com/y"];
    expect(dedupeUrls(urls)).toEqual(["https://a.com/x", "https://b.com/y"]);
  });

  it("treats a trailing-slash variant as the same URL", () => {
    const urls = ["https://a.com/x", "https://a.com/x/"];
    expect(dedupeUrls(urls)).toEqual(["https://a.com/x"]);
  });

  it("preserves first-seen order", () => {
    const urls = ["https://b.com/y", "https://a.com/x", "https://b.com/y"];
    expect(dedupeUrls(urls)).toEqual(["https://b.com/y", "https://a.com/x"]);
  });
});

describe("extensionOf / isAutoDetectableMedia", () => {
  it("extracts a simple extension", () => {
    expect(extensionOf("https://example.com/movie.mp4")).toBe("mp4");
  });

  it("ignores query strings and fragments", () => {
    expect(extensionOf("https://example.com/movie.mp4?token=abc#t=10")).toBe("mp4");
  });

  it("returns null when there's no extension", () => {
    expect(extensionOf("https://example.com/download")).toBeNull();
  });

  it("returns null for an invalid URL", () => {
    expect(extensionOf("not a url")).toBeNull();
  });

  it("recognizes known media/archive extensions", () => {
    expect(isAutoDetectableMedia("https://example.com/a.mp4")).toBe(true);
    expect(isAutoDetectableMedia("https://example.com/a.zip")).toBe(true);
    expect(isAutoDetectableMedia("https://example.com/a.html")).toBe(false);
  });
});

describe("shouldIntercept", () => {
  const baseSettings = {
    interceptEnabled: true,
    minSizeBytes: 5_000_000,
    fileTypeAllowlist: ["zip", "iso"],
    perSiteOptIn: [] as string[],
  };

  it("does nothing when interception is disabled entirely", () => {
    expect(
      shouldIntercept("https://example.com/a.zip", 10_000_000, {
        ...baseSettings,
        interceptEnabled: false,
      }),
    ).toBe(false);
  });

  it("intercepts when the file type is on the allowlist regardless of size", () => {
    expect(shouldIntercept("https://example.com/a.zip", 100, baseSettings)).toBe(true);
  });

  it("intercepts when the size threshold is met even with an unlisted extension", () => {
    expect(shouldIntercept("https://example.com/a.bin", 10_000_000, baseSettings)).toBe(true);
  });

  it("does not intercept a small, non-allowlisted file", () => {
    expect(shouldIntercept("https://example.com/a.bin", 1000, baseSettings)).toBe(false);
  });

  it("intercepts any download from a per-site opt-in host regardless of size/type", () => {
    expect(
      shouldIntercept("https://always-intercept.example.com/a.bin", 1, {
        ...baseSettings,
        perSiteOptIn: ["always-intercept.example.com"],
      }),
    ).toBe(true);
  });

  it("treats an unknown size as non-matching for the size threshold", () => {
    expect(shouldIntercept("https://example.com/a.bin", null, baseSettings)).toBe(false);
  });
});
