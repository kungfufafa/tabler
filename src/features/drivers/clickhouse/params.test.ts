/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  parseRoles,
  parseSettings,
  RESERVED_SETTINGS,
  settingProblem,
  toClickhouseParams,
} from "./params";

// The params' text forms, and the settings the form refuses, which must be the backend's list
// (`request::RESERVED`): a setting the form lets through would fail only at connect time, and
// one it refuses that the backend takes would block a valid profile.

const ROOT = path.resolve(import.meta.dirname, "../../../..");

/** The names of `const RESERVED: &[&str] = &[…];` in the backend's request builder. */
function backendReserved(): string[] {
  const src = readFileSync(path.join(ROOT, "src-tauri/src/drivers/clickhouse/request.rs"), "utf8");
  const block = /const RESERVED: &\[&str\] = &\[([\s\S]*?)\n\];/.exec(src)?.[1];
  if (!block) throw new Error("RESERVED not found in request.rs");
  return block
    .split("\n")
    .filter((line) => !line.trim().startsWith("//"))
    .flatMap((line) => [...line.matchAll(/"([^"]+)"/g)].map((m) => m[1]!));
}

describe("the connection settings", () => {
  it("refuse exactly the backend's reserved names", () => {
    const backend = backendReserved();
    expect(backend.length).toBeGreaterThan(40);
    expect([...RESERVED_SETTINGS].sort()).toEqual([...backend].sort());
  });

  it("refuse names as `validate_settings` does, in its words", () => {
    expect(settingProblem("max_threads")).toBeNull();
    expect(settingProblem(" Max_Threads ")).toBeNull();
    expect(settingProblem("max threads")).toBe("“max threads” is not a ClickHouse setting name");
    expect(settingProblem("")).toBe("“” is not a ClickHouse setting name");
    for (const name of ["READONLY", "param_id", "timeout_overflow_mode", "x_overflow_mode_leaf"])
      expect(settingProblem(name), name).toBe(
        `“${name}” is set by Tabler or changes how statements are read, so it cannot be a connection setting`,
      );
  });

  it("read `name = value` lines, keeping `=` in values", () => {
    expect(parseSettings("a = 1\r\n\n b=x=y \nbad\n=2")).toEqual({
      settings: [
        { name: "a", value: "1" },
        { name: "b", value: "x=y" },
      ],
      bad: [
        { line: 4, text: "bad" },
        { line: 5, text: "=2" },
      ],
    });
  });

  it("read roles at commas", () => {
    expect(parseRoles(" a, ,b ,c,")).toEqual(["a", "b", "c"]);
    expect(parseRoles("")).toEqual([]);
  });

  it("keep only what differs from the defaults", () => {
    expect(
      toClickhouseParams({
        certificateAuth: false,
        roles: " ",
        settings: "\n",
        sessionTimeoutSecs: "0",
      }),
    ).toBeNull();
  });
});
