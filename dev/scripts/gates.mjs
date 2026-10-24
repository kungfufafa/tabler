#!/usr/bin/env node
// dev/scripts/gates.mjs -- the foundation's gates while lanes work in the same checkout
// (docs/drivers/README.md). The plain gates read every file, so a lane's unfinished work fails
// them; these leave the lanes' files (dev/lanes.json) out and check the rest.
//
//   node dev/scripts/gates.mjs rs-fmt [--all]   rustfmt --check on the step's changed .rs files (every
//                                               shared one with --all), each on its own
//   node dev/scripts/gates.mjs rs               rs-fmt; in slot s0: clippy --all-targets (default and
//                                               --no-default-features), test --lib, and test with a
//                                               --test per shared suite
//   node dev/scripts/gates.mjs feat [--clippy]  cargo check (clippy) --all-targets --all-features on a
//                                               HEAD snapshot with the step's files over it, holding
//                                               every slot
//   node dev/scripts/gates.mjs ts               tsc, eslint, prettier and vitest without the lanes'
//                                               files, holding both TS slots
//
// "The step's files" are the changed files no lane owns. A lane's real-server suite is built only
// with its feature (`required-features`), so `--all-targets` skips it here, and the suites run by
// name; only rustfmt follows `mod` declarations whatever their cfg, which is why it runs file by
// file.

import fs from "node:fs";
import {
  ALL_SLOTS,
  GATE_SLOT,
  MANIFEST,
  ROOT,
  TOOLS,
  TS_SLOTS,
  UsageError,
  acquireAll,
  batches,
  capture,
  cargoEnv,
  gitStatus,
  nodeTool,
  ownerOf,
  readLanes,
  run,
  rustfmtFiles,
  sharedSuites,
  snapshot,
  statusPaths,
  tscCheck,
} from "./lane-lib.mjs";

/** The changed paths no lane owns (deleted ones included). */
async function stepFiles(lanes) {
  const status = await gitStatus();
  return [...new Set(status.flatMap(statusPaths))].filter((p) => ownerOf(lanes, p) === null);
}

async function rsFmt(lanes, all) {
  let files;
  if (all) {
    const { stdout } = await capture("git", [
      "ls-files",
      "--cached",
      "--others",
      "--exclude-standard",
      "--",
      "src-tauri/*.rs",
    ]);
    files = stdout.split(/\r?\n/).filter(Boolean);
  } else {
    files = await stepFiles(lanes);
  }
  files = files.filter(
    (f) =>
      f.startsWith("src-tauri/") &&
      f.endsWith(".rs") &&
      !f.startsWith("src-tauri/vendor/") &&
      ownerOf(lanes, f) === null,
  );
  const existing = files.filter((f) => fs.existsSync(`${ROOT}/${f}`));
  const { changed, failed } = await rustfmtFiles(existing);
  for (const f of changed) console.log(`[gates] not formatted: ${f}`);
  for (const f of failed) console.log(`[gates] rustfmt failed on ${f.file}:\n${f.message}`);
  console.log(`[gates] rustfmt: ${existing.length} shared file(s), ${changed.length} unformatted`);
  return changed.length || failed.length ? 1 : 0;
}

async function rs(lanes) {
  let failed = await rsFmt(lanes, false);
  await acquireAll([GATE_SLOT], "gates rs");
  // The lead's gate environment (s0, 8 jobs, the default debug info and bindings folder).
  const env = cargoEnv(GATE_SLOT, { jobs: 8, laneSlot: false });
  const m = ["--manifest-path", MANIFEST];
  const label = `gates ${GATE_SLOT}`;
  // The shared suites by name: once a lane lands in the default features its suite would build
  // under `--tests` too, and it is the lane's to run (`lane.mjs <lane> it`).
  const suites = sharedSuites(lanes).flatMap((name) => ["--test", name]);
  const steps = [
    ["clippy", ...m, "--all-targets", "--", "-D", "warnings"],
    ["clippy", ...m, "--all-targets", "--no-default-features", "--", "-D", "warnings"],
    ["test", ...m, "--lib"],
    ["test", ...m, ...suites],
  ];
  for (const args of steps) {
    if ((await run("cargo", args, { env, label })) !== 0) failed = 1;
  }
  const { stdout } = await capture("git", ["status", "--short", "--", "src/lib/ipc/bindings"]);
  if (stdout.trim()) console.log(`[gates] bindings changed by cargo test:\n${stdout}`);
  return failed;
}

async function feat(lanes, clippy) {
  // DuckDB's C++ next to everything else: no other build at the same time.
  await acquireAll(ALL_SLOTS, "gates feat (heavy)");
  const overlay = await stepFiles(lanes);
  const dir = await snapshot("foundation", overlay);
  console.log(`[gates] HEAD snapshot in ${dir} with ${overlay.length} step file(s)`);
  const env = cargoEnv(GATE_SLOT, { jobs: 6, laneSlot: false });
  const args = [
    clippy ? "clippy" : "check",
    "--manifest-path",
    MANIFEST,
    "--locked",
    "--all-targets",
    "--all-features",
    ...(clippy ? ["--", "-D", "warnings"] : []),
  ];
  return run("cargo", args, { cwd: dir, env, label: `gates ${GATE_SLOT} snapshot` });
}

async function ts(lanes) {
  await acquireAll(TS_SLOTS, "gates ts");
  let failed = 0;
  const laneTs = Object.values(lanes.lanes).flatMap((l) => l.ts);

  console.log("[gates] > tsc --noEmit (foundation filter)");
  const { failing, ignored } = await tscCheck(lanes, null);
  for (const d of failing) console.log(d.text);
  const others = Object.entries(ignored).map(([l, n]) => `${l} ${n}`);
  if (others.length) console.log(`[gates] ignored lane diagnostics: ${others.join(", ")}`);
  if (failing.length) failed = 1;

  const [eslint, eslintArgs] = nodeTool(TOOLS.eslint, [
    ".",
    ...laneTs.flatMap((g) => ["--ignore-pattern", g]),
  ]);
  if ((await run(eslint, eslintArgs)) !== 0) failed = 1;

  // prettier gets explicit lists: an extra ignore file would resolve its globs from its own folder.
  const { stdout } = await capture("git", [
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
  ]);
  const files = stdout
    .split(/\r?\n/)
    .filter((f) => f && ownerOf(lanes, f) === null && fs.existsSync(`${ROOT}/${f}`));
  for (const list of batches(files)) {
    const [cmd, args] = nodeTool(TOOLS.prettier, [
      "--check",
      "--end-of-line",
      "auto",
      "--ignore-unknown",
      "--log-level",
      "warn",
      ...list,
    ]);
    if ((await run(cmd, args)) !== 0) failed = 1;
  }

  const [vitest, vitestArgs] = nodeTool(TOOLS.vitest, [
    "run",
    ...laneTs.flatMap((g) => ["--exclude", g]),
  ]);
  if ((await run(vitest, vitestArgs)) !== 0) failed = 1;
  return failed;
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  const lanes = readLanes();
  const flag = (name) => rest.includes(name);
  const unknown = rest.filter((a) => !["--all", "--clippy"].includes(a));
  if (unknown.length) throw new UsageError(`unknown argument ${unknown[0]}`);
  switch (command) {
    case "rs-fmt":
      return rsFmt(lanes, flag("--all"));
    case "rs":
      return rs(lanes);
    case "feat":
      return feat(lanes, flag("--clippy"));
    case "ts":
      return ts(lanes);
    default:
      throw new UsageError("usage: gates.mjs <rs-fmt [--all] | rs | feat [--clippy] | ts>");
  }
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`[gates] ${e instanceof UsageError ? "" : "error: "}${e.message}`);
    process.exit(e instanceof UsageError ? 2 : 1);
  },
);
