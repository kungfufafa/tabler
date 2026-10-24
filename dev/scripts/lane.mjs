#!/usr/bin/env node
// dev/scripts/lane.mjs -- builds and tests one driver lane on its own (docs/drivers/README.md).
//
//   node dev/scripts/lane.mjs <lane> check          cargo check --all-targets, the lane's features only
//   node dev/scripts/lane.mjs <lane> clippy         cargo clippy --all-targets -- -D warnings
//   node dev/scripts/lane.mjs <lane> test [filter…] lib tests: the lane's filters, or the ones given
//   node dev/scripts/lane.mjs <lane> test-all       every lib test, with the lane's features
//   node dev/scripts/lane.mjs <lane> it             the lane's real-server suites (loads .env)
//   node dev/scripts/lane.mjs <lane> bindings       exports the bindings in the slot, copies the lane's
//   node dev/scripts/lane.mjs <lane> ts             tsc-lane, eslint, prettier, vitest (own folders + smoke)
//   node dev/scripts/lane.mjs <lane> fmt            rustfmt and prettier --write on the lane's files
//
// Options: --slot sN builds in that slot; --heavy also holds every other slot, s0 included (a
// cold DuckDB build); --jobs N; --snapshot builds HEAD with the lane's changed files over it
// (check, clippy, test, test-all) instead of the working tree.
//
// Cargo always runs with `--locked --no-default-features --features <the lane's>`, so the other
// lanes' code is compiled out, in a slot under C:/t with CARGO_BUILD_JOBS=4. Without --slot the
// lane takes its home slot (dev/lanes.json) or, while that is busy, any free lane slot, and waits
// when all are. The renderer commands hold one of two TS slots.

import fs from "node:fs";
import {
  ALL_SLOTS,
  LANE_SLOTS,
  MANIFEST,
  ROOT,
  TOOLS,
  TS_SLOTS,
  UsageError,
  acquireAll,
  acquireAny,
  cargoEnv,
  existingDirs,
  filesOf,
  getLane,
  gitStatus,
  globMatches,
  laneFeatureArgs,
  nodeTool,
  ownerOf,
  readEnvFile,
  readLanes,
  run,
  rustfmtFiles,
  slotDir,
  snapshot,
  statusPaths,
  tscCheck,
  walk,
} from "./lane-lib.mjs";

const COMMANDS = ["check", "clippy", "test", "test-all", "it", "bindings", "ts", "fmt"];
const CARGO_COMMANDS = ["check", "clippy", "test", "test-all", "it", "bindings"];
const BINDINGS_DIR = "src/lib/ipc/bindings";

function parseArgs(argv) {
  const opts = { rest: [], heavy: false, snapshot: false, slot: null, jobs: null };
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--heavy") opts.heavy = true;
    else if (a === "--snapshot") opts.snapshot = true;
    else if (a === "--slot" || a === "--jobs") {
      const value = argv[++i];
      if (!value) throw new UsageError(`${a} needs a value`);
      opts[a.slice(2)] = value;
    } else if (a === "--") (positional.push(...argv.slice(i + 1)), (i = argv.length));
    else if (a.startsWith("--")) throw new UsageError(`unknown option ${a}`);
    else positional.push(a);
  }
  const [lane, command, ...rest] = positional;
  if (!lane || !command) throw new UsageError(`usage: lane.mjs <lane> <${COMMANDS.join("|")}>`);
  if (!COMMANDS.includes(command)) throw new UsageError(`unknown command ${command}`);
  if (opts.slot && !LANE_SLOTS.includes(opts.slot) && opts.slot !== "s0") {
    throw new UsageError(`--slot: one of ${ALL_SLOTS.join(", ")}`);
  }
  if (opts.snapshot && !["check", "clippy", "test", "test-all"].includes(command)) {
    throw new UsageError("--snapshot goes with check, clippy, test and test-all");
  }
  return { lane, command, rest, ...opts };
}

/** Takes a build slot for the lane; returns the one it builds in. */
async function takeSlot(lane, command, opts) {
  const what = `${lane.name} ${command}`;
  const preferred = opts.slot ?? lane.slot;
  if (opts.heavy) {
    // Everything, in one fixed order; the build itself stays in the lane's slot.
    await acquireAll(ALL_SLOTS, `${what} (heavy)`);
    return preferred;
  }
  if (opts.slot) return acquireAny([opts.slot], what);
  return acquireAny([preferred, ...LANE_SLOTS.filter((s) => s !== preferred)], what);
}

/** The lane's changed paths (what a lane commit would take). */
async function laneChanges(lanes, lane) {
  const status = await gitStatus();
  return [...new Set(status.flatMap(statusPaths))].filter((p) => ownerOf(lanes, p) === lane.name);
}

async function cargo(lanes, lane, command, opts) {
  const slot = await takeSlot(lane, command, opts);
  const jobs = Number(opts.jobs ?? (opts.heavy ? 6 : 4));
  const env = cargoEnv(slot, { jobs, lane: lane.name });
  let cwd = ROOT;
  if (opts.snapshot) {
    const overlay = await laneChanges(lanes, lane);
    cwd = await snapshot(`lane-${lane.name}`, overlay);
    console.log(`[${lane.name}] HEAD snapshot in ${cwd} with ${overlay.length} lane file(s)`);
  }
  const base = laneFeatureArgs(lane, MANIFEST);
  const label = `${lane.name} ${slot}`;
  const skip = ["--skip", "export_bindings_"];
  switch (command) {
    case "check":
      return run("cargo", ["check", ...base, "--all-targets"], { cwd, env, label });
    case "clippy":
      return run("cargo", ["clippy", ...base, "--all-targets", "--", "-D", "warnings"], {
        cwd,
        env,
        label,
      });
    case "test": {
      const filters = opts.rest.length ? opts.rest : lane.tests;
      return run("cargo", ["test", ...base, "--lib", "--", ...skip, ...filters], {
        cwd,
        env,
        label,
      });
    }
    case "test-all":
      return run("cargo", ["test", ...base, "--lib", "--", ...skip], { cwd, env, label });
    case "it":
      return integration(lane, base, env, label);
    case "bindings":
      return bindings(lane, base, slot, env, label);
  }
}

/** The lane's suites in src-tauri/tests, with `.env` loaded under the variables already set. */
async function integration(lane, base, env, label) {
  const suites = lane.rust.flatMap((g) => /^src-tauri\/tests\/(\w+)\.rs$/.exec(g)?.[1] ?? []);
  const fromFile = readEnvFile(`${ROOT}/.env`);
  const merged = { ...fromFile, ...env };
  for (const name of lane.env) {
    console.log(`[${label}] ${name} ${merged[name] ? "set" : "unset: its tests skip"}`);
  }
  const tests = suites.flatMap((s) => ["--test", s]);
  return run("cargo", ["test", ...base, ...tests, "--", "--include-ignored", "--test-threads=1"], {
    env: merged,
    label,
  });
}

/**
 * Exports every binding into the slot, then copies the lane's own into src/lib/ipc/bindings
 * and removes the lane's files there that are no longer exported. Shared bindings are never
 * touched.
 */
async function bindings(lane, base, slot, env, label) {
  const out = `${slotDir(slot)}/bindings-${lane.name}`;
  fs.rmSync(out, { recursive: true, force: true });
  const code = await run("cargo", ["test", ...base, "--lib", "--", "export_bindings_"], {
    env,
    label,
  });
  if (code !== 0) return code;
  const own = (file) => lane.bindings.some((g) => globMatches(`${BINDINGS_DIR}/${file}`, g));
  const exported = walk(out).filter(own);
  const current = filesOf(lane.bindings).map((f) => f.slice(BINDINGS_DIR.length + 1));
  for (const file of exported) {
    const to = `${ROOT}/${BINDINGS_DIR}/${file}`;
    fs.mkdirSync(to.slice(0, to.lastIndexOf("/")), { recursive: true });
    fs.copyFileSync(`${out}/${file}`, to);
  }
  const stale = current.filter((f) => !exported.includes(f));
  for (const file of stale) fs.rmSync(`${ROOT}/${BINDINGS_DIR}/${file}`);
  console.log(
    `[${label}] bindings: ${exported.length} copied${stale.length ? `, ${stale.length} removed (${stale.join(", ")})` : ""}`,
  );
  return 0;
}

/** tsc-lane, then eslint, prettier and vitest on the lane's renderer files. */
async function ts(lanes, lane) {
  await acquireAny(TS_SLOTS, `${lane.name} ts`);
  const label = `${lane.name} ts`;
  let failed = 0;

  console.log(`[${label}] > tsc --noEmit (lane filter)`);
  const { failing, ignored } = await tscCheck(lanes, lane.name);
  for (const d of failing) console.log(d.text);
  const others = Object.entries(ignored).map(([l, n]) => `${l} ${n}`);
  if (others.length)
    console.log(`[${label}] ignored other lanes' diagnostics: ${others.join(", ")}`);
  if (failing.length) {
    console.log(`[${label}] tsc: ${failing.length} diagnostic(s) in shared or ${lane.name} files`);
    failed++;
  }

  const dirs = existingDirs(lane.ts);
  if (dirs.length) {
    const [cmd, args] = nodeTool(TOOLS.eslint, ["--no-warn-ignored", ...dirs]);
    if ((await run(cmd, args, { label })) !== 0) failed++;
  }
  const formatted = [...dirs, ...filesOf(lane.docs)];
  if (formatted.length) {
    const [cmd, args] = nodeTool(TOOLS.prettier, [
      "--check",
      "--end-of-line",
      "auto",
      "--ignore-unknown",
      ...formatted,
    ]);
    if ((await run(cmd, args, { label })) !== 0) failed++;
  }

  // Its own folders and the shared smoke set; every other lane's folders stay out.
  const exclude = Object.entries(lanes.lanes)
    .filter(([name]) => name !== lane.name)
    .flatMap(([, l]) => l.ts.flatMap((g) => ["--exclude", g]));
  const smoke = lanes.smoke.filter((f) => fs.existsSync(`${ROOT}/${f}`));
  const [cmd, args] = nodeTool(TOOLS.vitest, [
    "run",
    "--maxWorkers=2",
    "--passWithNoTests",
    ...exclude,
    ...lane.ts.map((g) => g.replace(/\/\*\*$/, "/")),
    ...smoke,
  ]);
  if ((await run(cmd, args, { label })) !== 0) failed++;
  return failed ? 1 : 0;
}

/** rustfmt (file by file) and prettier --write on the lane's own files. */
async function fmt(lane) {
  const label = `${lane.name} fmt`;
  const rs = filesOf(lane.rust).filter((f) => f.endsWith(".rs"));
  const { changed, failed } = await rustfmtFiles(rs, { write: true });
  console.log(`[${label}] rustfmt: ${rs.length} file(s), ${changed.length} rewritten`);
  for (const f of failed) console.log(`[${label}] rustfmt failed on ${f.file}:\n${f.message}`);
  const formatted = [...existingDirs(lane.ts), ...filesOf(lane.docs)];
  let code = failed.length ? 1 : 0;
  if (formatted.length) {
    // Each file keeps its line endings (CRLF in a checkout with core.autocrlf), as the check does.
    const [cmd, args] = nodeTool(TOOLS.prettier, [
      "--write",
      "--end-of-line",
      "auto",
      "--ignore-unknown",
      ...formatted,
    ]);
    if ((await run(cmd, args, { label })) !== 0) code = 1;
  }
  return code;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const lanes = readLanes();
  const lane = getLane(lanes, opts.lane);
  if (CARGO_COMMANDS.includes(opts.command)) return cargo(lanes, lane, opts.command, opts);
  if (opts.command === "ts") return ts(lanes, lane);
  return fmt(lane);
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`[lane] ${e instanceof UsageError ? "" : "error: "}${e.message}`);
    process.exit(e instanceof UsageError ? 2 : 1);
  },
);
