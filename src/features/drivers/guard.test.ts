/// <reference types="node" />
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { DESCRIPTORS, KINDS } from "./catalog";

/**
 * Guard of the driver lanes in the renderer (G4; `src-tauri/tests/driver_guard.rs` is the
 * backend's). The drivers beyond PostgreSQL, MySQL and SQLite and the PostgreSQL flavors are named
 * (by their quoted ids or as object keys) only where naming them is the point: the catalog and
 * the lanes (`src/features/drivers`), the mock's lanes (`src/lib/ipc/mock/drivers`), the
 * generated bindings and tests. Elsewhere code reads the catalog, so a new driver needs no edit
 * there.
 *
 * Shared code also hides no driver behind a cast (`as Driver`, `as Dialect`…) or a
 * `Record<string, Dialect>` lookup, which turn another engine into a core one without a type
 * error; and grows no new `driver === "…"` compares: their count per file stays at or below
 * `guardBaseline.json` (`TABLER_BLESS=1` rewrites it after a reviewed change). Comments count too.
 */

const ROOT = path.resolve(import.meta.dirname, "../../..");
const BASELINE = path.join(import.meta.dirname, "guardBaseline.json");

/** Folders that name every driver: the catalog with the lanes, and the mock's lanes. */
const LANE_DIRS = ["src/features/drivers/", "src/lib/ipc/mock/drivers/"];
const BINDINGS = "src/lib/ipc/bindings/";

const LANE_DRIVERS = Object.values(DESCRIPTORS)
  .filter((d) => d.cargoFeature !== null)
  .map((d) => d.driver);
const FLAVORS = KINDS.flatMap((k) => (k.pgFlavor ? [k.pgFlavor] : []));

/** Every `.ts` / `.tsx` file under `src`, as `src/…` paths with `/`. */
function sourceFiles(): string[] {
  return readdirSync(path.join(ROOT, "src"), { recursive: true, encoding: "utf8" })
    .filter((f) => /\.tsx?$/.test(f))
    .map((f) => `src/${f.split(path.sep).join("/")}`)
    .sort();
}

const isTest = (file: string) => /\.test\.tsx?$/.test(file) || file.startsWith("src/test/");
const inLane = (file: string) => LANE_DIRS.some((d) => file.startsWith(d));

/** Where `text` names one of `ids`: quoted, or as an object key (`redis: …`). */
function namedIds(text: string, ids: readonly string[]): Array<{ id: string; index: number }> {
  const alt = ids.join("|");
  const re = new RegExp(`(["'\`])(${alt})\\1|^\\s*(${alt})\\??\\s*:`, "gm");
  return [...text.matchAll(re)].map((m) => ({ id: m[2] ?? m[3]!, index: m.index }));
}

/** Casts to a driver or dialect type, and string-keyed records of them. */
function hidingCasts(text: string): string[] {
  const re =
    /\bas\s+(?:readonly\s+)?(?:Dialect|SqlDialect|SqlDialectId|Driver|CoreDriver|DatabaseKind|PgFlavor)\b|Record<\s*string\s*,\s*(?:Dialect|SqlDialect|SqlDialectId|Driver|DatabaseKind)\s*>/g;
  return [...text.matchAll(re)].map((m) => m[0]);
}

/** `driver === "…"` and `driver !== "…"` compares (any `.driver` too). */
function driverCompares(text: string): number {
  return [...text.matchAll(/\bdriver\s*[!=]==\s*["'`]/g)].length;
}

const lineOf = (text: string, index: number) => text.slice(0, index).split("\n").length;

describe("the renderer's driver guard (G4)", () => {
  const files = sourceFiles().map((file) => ({
    file,
    text: readFileSync(path.join(ROOT, file), "utf8"),
  }));

  it("knows the 13 lane drivers and the 3 PostgreSQL flavors from the catalog", () => {
    expect(LANE_DRIVERS).toHaveLength(13);
    expect(FLAVORS).toHaveLength(3);
    expect(files.length).toBeGreaterThan(500);
  });

  it("names the lane drivers and flavors only in the lanes, the bindings and tests", () => {
    const ids = [...LANE_DRIVERS, ...FLAVORS];
    const found: string[] = [];
    for (const { file, text } of files) {
      if (inLane(file) || file.startsWith(BINDINGS) || isTest(file)) continue;
      for (const { id, index } of namedIds(text, ids))
        found.push(`${file}:${lineOf(text, index)} ${id}`);
    }
    expect(found, "read the catalog (src/features/drivers/catalog.ts) instead").toEqual([]);
  });

  it("hides no driver behind a cast or a string-keyed record", () => {
    const found: string[] = [];
    for (const { file, text } of files) {
      if (file.startsWith(BINDINGS) || isTest(file)) continue;
      for (const cast of hidingCasts(text)) found.push(`${file}: ${cast}`);
    }
    expect(found, "narrow with coreDialect / isCoreDriver or a lookup instead").toEqual([]);
  });

  it("grows no driver compares in shared code", () => {
    const counts: Record<string, number> = {};
    for (const { file, text } of files) {
      if (inLane(file) || file.startsWith(BINDINGS) || isTest(file)) continue;
      const n = driverCompares(text);
      if (n > 0) counts[file] = n;
    }
    if (process.env.TABLER_BLESS) {
      writeFileSync(BASELINE, `${JSON.stringify({ driverCompares: counts }, null, 2)}\n`);
      return;
    }
    const baseline = JSON.parse(readFileSync(BASELINE, "utf8")) as {
      driverCompares: Record<string, number>;
    };
    const grown = Object.entries(counts)
      .filter(([file, n]) => n > (baseline.driverCompares[file] ?? 0))
      .map(([file, n]) => `${file}: ${n} (baseline ${baseline.driverCompares[file] ?? 0})`);
    expect(
      grown,
      "read the catalog instead, or bless a reviewed change with TABLER_BLESS=1",
    ).toEqual([]);
  });

  it("finds what it guards against", () => {
    const sample = [
      `const a = "redis"; const b = 'cockroachdb'; const c = \`duckdb\`;`,
      `const LABEL = {`,
      `  mongodb: "MongoDB",`,
      `  d1?: 1,`,
      `};`,
      `// redis in a comment and a "redshiftish" string are not ids`,
      `const d = info.driver as Driver; const e = x as readonly Dialect[];`,
      `const f: Record<string, SqlDialect> = {}; const g = y as DatabaseKind;`,
      `const h: Record<string, string> = {}; const i = z as CoreDriverish;`,
      `if (info.driver === "mysql" || driver !== 'sqlite' || kind === "mariadb") {}`,
    ].join("\n");
    expect(namedIds(sample, [...LANE_DRIVERS, ...FLAVORS]).map((m) => m.id)).toEqual([
      "redis",
      "cockroachdb",
      "duckdb",
      "mongodb",
      "d1",
    ]);
    expect(hidingCasts(sample)).toEqual([
      "as Driver",
      "as readonly Dialect",
      "Record<string, SqlDialect>",
      "as DatabaseKind",
    ]);
    expect(driverCompares(sample)).toBe(2);
  });
});
