/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { LISTS, PLUGIN_FILES_REFUSED, readsFiles } from "./files";

// The mock's `readsFiles` against the backend's `dialect::duckdb::reads_files`: the shared cases
// (`tests/fixtures/duckdb/plugin-files.json`, which the Rust test runs too), the word lists and
// the refusal's words.

const ROOT = path.resolve(import.meta.dirname, "../../../..");
const read = (p: string) => readFileSync(path.join(ROOT, p), "utf8");

describe("a plugin's run on a DuckDB session that is not hardened", () => {
  it("is refused and admitted as the backend's shared cases say", () => {
    const cases = JSON.parse(read("src-tauri/tests/fixtures/duckdb/plugin-files.json")) as {
      refused: string[];
      admitted: string[];
    };
    expect(cases.refused.length).toBeGreaterThan(50);
    expect(cases.refused.filter((sql) => !readsFiles(sql))).toEqual([]);
    expect(cases.admitted.filter((sql) => readsFiles(sql))).toEqual([]);
  });

  it("keeps the backend's word lists and words", () => {
    const rs = read("src-tauri/src/sqlgen/dialect/duckdb.rs");
    for (const [name, list] of Object.entries(LISTS)) {
      const m = new RegExp(`const ${name}: \\[&str; (\\d+)\\] = \\[([^\\]]*)\\];`).exec(rs);
      expect(m, name).not.toBeNull();
      const words = [...m![2]!.matchAll(/"([^"]*)"/g)].map((w) => w[1]);
      expect(words, name).toEqual(list);
      expect(Number(m![1]), name).toBe(list.length);
    }
    const session = read("src-tauri/src/drivers/duckdb/session.rs").replace(/\\\r?\n\s*/g, "");
    expect(session).toContain(`"${PLUGIN_FILES_REFUSED}"`);
  });
});
