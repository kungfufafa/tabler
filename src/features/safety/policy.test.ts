import { describe, expect, it } from "vitest";
import type { SafeModeLevel } from "@/lib/ipc";
import { SAFE_MODE_LEVELS } from "@/stores/prefs";
import { effectiveLevel, lowersSafeLevel, SAFE_MODE_LABEL, SAFE_MODE_OPTIONS } from "./levels";
import { decide, requestWrites, type GuardDecision } from "./policy";

describe("decide", () => {
  const table: [SafeModeLevel, read: GuardDecision, write: GuardDecision][] = [
    ["silent", "go", "go"],
    ["alert", "alert", "alert"],
    ["alertNonSelect", "go", "alert"],
    ["safe", "password", "password"],
    ["safeNonSelect", "go", "password"],
  ];
  it.each(table)("%s: reads → %s, writes → %s", (level, read, write) => {
    expect(decide(level, false, false)).toBe(read);
    expect(decide(level, true, false)).toBe(write);
  });

  it("refuses writes on a read-only connection whatever the level; reads follow the level", () => {
    for (const [level, read] of table) {
      expect(decide(level, true, true)).toBe("blocked");
      expect(decide(level, false, true)).toBe(read);
    }
  });
});

describe("requestWrites", () => {
  it("classifies queries, always counts commits, trusts operations", () => {
    expect(requestWrites({ kind: "query", statements: ["SELECT 1", "SHOW x"] }, "mysql")).toBe(
      false,
    );
    expect(
      requestWrites({ kind: "query", statements: ["SELECT 1", "DROP TABLE t"] }, "mysql"),
    ).toBe(true);
    expect(requestWrites({ kind: "query", statements: [] }, "postgres")).toBe(true);
    expect(requestWrites({ kind: "commit", statements: ["SELECT 1"] }, "postgres")).toBe(true);
    expect(requestWrites({ kind: "operation", label: "Back up", write: false }, "sqlite")).toBe(
      false,
    );
    expect(requestWrites({ kind: "operation", label: "Restore", write: true }, "sqlite")).toBe(
      true,
    );
  });
});

describe("levels", () => {
  it("lists TablePlus's five levels in menu order", () => {
    expect(SAFE_MODE_OPTIONS.map((o) => o.level)).toEqual([...SAFE_MODE_LEVELS]);
    expect(Object.values(SAFE_MODE_LABEL)).toEqual([
      "Silent Mode",
      "Alert Mode 1",
      "Alert Mode 2",
      "Safe Mode 1",
      "Safe Mode 2",
    ]);
    expect(effectiveLevel(null)).toBe("silent");
    expect(effectiveLevel(undefined)).toBe("silent");
    expect(effectiveLevel("safe")).toBe("safe");
  });

  it("asks for the password only when leaving a Safe level for one that asks less", () => {
    expect(lowersSafeLevel("safe", "silent")).toBe(true);
    expect(lowersSafeLevel("safe", "safeNonSelect")).toBe(true);
    expect(lowersSafeLevel("safeNonSelect", "alert")).toBe(true);
    expect(lowersSafeLevel("safeNonSelect", "safe")).toBe(false);
    expect(lowersSafeLevel("alert", "silent")).toBe(false);
    expect(lowersSafeLevel("silent", "safe")).toBe(false);
    expect(lowersSafeLevel("safe", "safe")).toBe(false);
  });
});
