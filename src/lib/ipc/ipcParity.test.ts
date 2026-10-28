/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { mockHandler } from "./mock/transport";
import "./mock";

// Every command the backend registers has its facade call and its mock, and nothing else does:
// a new command without a mock would only fail in `vite --mode mock` and in the tests.

const root = path.resolve(import.meta.dirname, "../../..");
const facade = readFileSync(path.join(root, "src/lib/ipc/index.ts"), "utf8");
const lib = readFileSync(path.join(root, "src-tauri/src/lib.rs"), "utf8");

/** The command names of the facade's `invoke("…")` calls. */
const invoked = [...facade.matchAll(/\binvoke\b[^(\n]*\(\s*"([a-z_0-9]+)"/g)].map((m) => m[1]!);

/** The commands of `tauri::generate_handler![…]`. */
function registered(): string[] {
  const start = lib.indexOf("generate_handler![");
  const block = lib.slice(start, lib.indexOf("]", start));
  return [...block.matchAll(/commands::\w+::(\w+)/g)].map((m) => m[1]!);
}

describe("IPC parity", () => {
  it("calls each registered command from the facade, and only those", () => {
    const commands = registered();
    expect(commands.length).toBeGreaterThan(100);
    expect(new Set(commands).size).toBe(commands.length);
    expect([...new Set(invoked)].sort()).toEqual([...commands].sort());
  });

  it("has a mock for every command the facade calls", () => {
    const missing = invoked.filter((cmd) => mockHandler(cmd) === undefined);
    expect(missing).toEqual([]);
  });
});
