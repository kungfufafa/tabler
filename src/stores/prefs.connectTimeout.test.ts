import { describe, expect, it } from "vitest";
import { CONNECT_TIMEOUT_RANGE, DEFAULT_PREFS, sanitizePrefs } from "./prefs";

describe("connect timeout preference", () => {
  it("defaults to 60 seconds, as TablePlus's", () => {
    expect(DEFAULT_PREFS.connectTimeout).toBe(60);
  });

  it("stays within 1 to 300 seconds, and falls back to the default outside", () => {
    expect(CONNECT_TIMEOUT_RANGE).toEqual({ min: 1, max: 300 });
    expect(DEFAULT_PREFS.connectTimeout).toBeGreaterThanOrEqual(CONNECT_TIMEOUT_RANGE.min);
    expect(DEFAULT_PREFS.connectTimeout).toBeLessThanOrEqual(CONNECT_TIMEOUT_RANGE.max);
    expect(sanitizePrefs({ connectTimeout: 0 }).connectTimeout).toBe(60);
    expect(sanitizePrefs({ connectTimeout: 301 }).connectTimeout).toBe(60);
    expect(sanitizePrefs({ connectTimeout: 120 }).connectTimeout).toBe(120);
  });
});
