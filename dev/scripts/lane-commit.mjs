#!/usr/bin/env node
// dev/scripts/lane-commit.mjs -- commits a lane's own files and nothing else
// (docs/drivers/README.md).
//
//   node dev/scripts/lane-commit.mjs <lane> -m "<lane>: …" [-m <paragraph>…] [--snapshot] [-- <path>…]
//
// Without paths it takes every changed file the lane owns (dev/lanes.json); paths given must all
// be the lane's (a folder stands for its changed files). New files are added, then
// `git commit --only` records exactly these paths, whatever else is staged or changed.
//
// A lane builds against the working tree, so while shared files a build reads are changed and
// uncommitted (foundation work in progress), its commit could depend on them. It is refused then,
// unless --snapshot: the lane is then checked (cargo check --all-targets with its features, and
// tsc) on a copy of HEAD with only the files being committed over it, and committed if that
// passes.

import fs from "node:fs";
import os from "node:os";
import {
  LANE_SLOTS,
  MANIFEST,
  ROOT,
  TS_SLOTS,
  UsageError,
  acquireAny,
  capture,
  cargoEnv,
  getLane,
  gitStatus,
  isBuildInput,
  laneFeatureArgs,
  ownerOf,
  readLanes,
  release,
  repoPath,
  run,
  snapshot,
  statusPaths,
  tscCheck,
} from "./lane-lib.mjs";

function parseArgs(argv) {
  const opts = { lane: null, messages: [], snapshot: false, paths: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--") {
      opts.paths = argv.slice(i + 1);
      break;
    } else if (a === "-m" || a === "--message") {
      const value = argv[++i];
      if (value === undefined) throw new UsageError(`${a} needs a value`);
      opts.messages.push(value);
    } else if (a === "--snapshot") opts.snapshot = true;
    else if (a.startsWith("-")) throw new UsageError(`unknown option ${a}`);
    else if (!opts.lane) opts.lane = a;
    else throw new UsageError(`unexpected ${a} (paths go after --)`);
  }
  if (!opts.lane) {
    throw new UsageError('usage: lane-commit.mjs <lane> -m "<message>" [--snapshot] [-- <path>…]');
  }
  return opts;
}

/** The changed paths to commit: the given ones (all the lane's) or all the lane's. */
function selectPaths(lanes, lane, changed, given) {
  if (!given) return changed.filter((p) => ownerOf(lanes, p) === lane);
  const out = new Set();
  const refused = [];
  for (const raw of given) {
    const p = repoPath(raw).replace(/\/$/, "");
    if (p.startsWith("..")) {
      refused.push(`${raw} (outside the repository)`);
      continue;
    }
    // A file, or a folder standing for its changed files.
    const hits = changed.filter((c) => c === p || c.startsWith(`${p}/`));
    const probe = fs.statSync(`${ROOT}/${p}`, { throwIfNoEntry: false })?.isDirectory()
      ? `${p}/`
      : p;
    const owners = new Set((hits.length ? hits : [probe]).map((c) => ownerOf(lanes, c)));
    if (owners.size !== 1 || !owners.has(lane)) {
      const who = [...owners].map((o) => o ?? "shared").join(", ");
      refused.push(`${p} (${who})`);
      continue;
    }
    if (!hits.length) console.log(`[lane-commit] ${p}: no changes, skipped`);
    for (const h of hits) out.add(h);
  }
  if (refused.length) {
    throw new UsageError(
      `refused: not owned by the ${lane} lane:\n  ${refused.join("\n  ")}\n` +
        "Shared files are the lead's: ask for the change instead (docs/drivers/README.md).",
    );
  }
  return [...out].sort();
}

/** Runs git, retrying while another git process holds index.lock. */
async function git(args) {
  for (let attempt = 1; ; attempt++) {
    const { code, stdout, stderr } = await capture("git", args);
    if (code === 0 || !stderr.includes("index.lock") || attempt === 15) {
      process.stdout.write(stdout);
      process.stderr.write(stderr);
      return code;
    }
    console.error("[lane-commit] index.lock is held, retrying");
    await new Promise((r) => setTimeout(r, 1000));
  }
}

async function author() {
  const name = (await capture("git", ["config", "user.name"])).stdout.trim();
  const email = (await capture("git", ["config", "user.email"])).stdout.trim();
  if (!name || !email) {
    throw new UsageError(
      "git has no author (user.name / user.email): ask the lead, who sets it for the repository",
    );
  }
}

/** The lane checked on HEAD plus `paths`: cargo check with its features, then tsc. */
async function verifyOnSnapshot(lanes, lane, paths) {
  const slot = await acquireAny(
    [lane.slot, ...LANE_SLOTS.filter((s) => s !== lane.slot)],
    `${lane.name} commit`,
  );
  const dir = await snapshot(`commit-${lane.name}`, paths, { nodeModules: true });
  const label = `${lane.name} ${slot} snapshot`;
  const code = await run("cargo", ["check", ...laneFeatureArgs(lane, MANIFEST), "--all-targets"], {
    cwd: dir,
    env: cargoEnv(slot, { jobs: 4, lane: lane.name }),
    label,
  });
  release(slot);
  if (code !== 0) return false;
  const ts = await acquireAny(TS_SLOTS, `${lane.name} commit tsc`);
  console.log(`[${label}] > tsc --noEmit (lane filter)`);
  const { failing } = await tscCheck(lanes, lane.name, dir);
  release(ts);
  for (const d of failing) console.log(d.text);
  return failing.length === 0;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const lanes = readLanes();
  const lane = getLane(lanes, opts.lane);
  const status = await gitStatus();
  const changed = [...new Set(status.flatMap(statusPaths))];
  const paths = selectPaths(lanes, lane.name, changed, opts.paths);
  if (!opts.messages.length) throw new UsageError("a commit message is needed (-m)");
  if (!paths.length) throw new UsageError(`nothing of the ${lane.name} lane to commit`);
  await author();

  const shared = changed.filter((p) => ownerOf(lanes, p) === null && isBuildInput(p));
  if (shared.length) {
    const list = shared.slice(0, 20).join("\n  ") + (shared.length > 20 ? "\n  …" : "");
    if (!opts.snapshot) {
      throw new UsageError(
        `refused: shared files a build reads are changed and not committed:\n  ${list}\n` +
          "Wait for them to be committed, or pass --snapshot to check the lane on HEAD plus " +
          "its own files first.",
      );
    }
    console.log(`[lane-commit] shared files changed, checking on a HEAD snapshot:\n  ${list}`);
    if (!(await verifyOnSnapshot(lanes, lane, paths))) {
      console.error(
        "[lane-commit] the lane does not build on HEAD with only its files: not committed",
      );
      return 1;
    }
  }

  const tracked = new Set(status.filter((e) => e.x !== "?").flatMap(statusPaths));
  const untracked = paths.filter((p) => !tracked.has(p) && fs.existsSync(`${ROOT}/${p}`));
  const list = `${os.tmpdir()}/tabler-lane-commit-${process.pid}.txt`;
  try {
    if (untracked.length) {
      fs.writeFileSync(list, untracked.join("\0"));
      const added = await git([
        "--literal-pathspecs",
        "add",
        `--pathspec-from-file=${list}`,
        "--pathspec-file-nul",
      ]);
      if (added !== 0) return added;
    }
    fs.writeFileSync(list, paths.join("\0"));
    const messages = opts.messages.flatMap((m) => ["-m", m]);
    const code = await git([
      "--literal-pathspecs",
      "commit",
      "--only",
      ...messages,
      `--pathspec-from-file=${list}`,
      "--pathspec-file-nul",
    ]);
    if (code === 0) console.log(`[lane-commit] ${lane.name}: ${paths.length} path(s) committed`);
    return code;
  } finally {
    fs.rmSync(list, { force: true });
  }
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`[lane-commit] ${e instanceof UsageError ? "" : "error: "}${e.message}`);
    process.exit(e instanceof UsageError ? 2 : 1);
  },
);
