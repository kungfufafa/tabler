// dev/scripts/lane-lib.mjs -- what lane.mjs, tsc-lane.mjs, lane-commit.mjs and gates.mjs share
// (see docs/drivers/README.md): the lanes of dev/lanes.json and who owns a path, the cargo
// environment of a build slot, the slot locks, the HEAD snapshot, and running commands.
//
// Build slots are short target folders under C:/t (TABLER_SLOT_ROOT elsewhere): s1..s3 for the
// lanes, s0 for the foundation's gates, never the folder `tauri dev` builds in. A lock file in a
// slot lets one build at a time use it; the renderer tools take one of two TS slots the same way,
// so the lanes' builds and test runs never outgrow the machine's memory together.

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const posix = (p) => p.split(path.sep).join("/");

/** The repository root, with `/`. */
export const ROOT = posix(path.resolve(fileURLToPath(new URL("../..", import.meta.url))));
export const MANIFEST = "src-tauri/Cargo.toml";

/** Thrown for a wrong command line: the scripts print the message and exit with 2. */
export class UsageError extends Error {}

// --- Lanes and ownership --------------------------------------------------------------------

export function readLanes(root = ROOT) {
  return JSON.parse(fs.readFileSync(`${root}/dev/lanes.json`, "utf8"));
}

/** The lane called `name`, with its name. */
export function getLane(lanes, name) {
  const lane = lanes.lanes[name];
  if (!lane) {
    const names = Object.keys(lanes.lanes).join(", ");
    throw new UsageError(`unknown lane "${name}" (one of: ${names})`);
  }
  return { name, ...lane };
}

/** Every glob a lane owns: its Rust and renderer code, its bindings and its docs. */
export function ownedGlobs(lane) {
  return [...lane.rust, ...lane.ts, ...lane.bindings, ...lane.docs];
}

/**
 * Whether `file` (a repository path with `/`) matches `glob`: `dir/**` is everything under the
 * folder, a `*` stands for any rest of one name, anything else names one file.
 */
export function globMatches(file, glob) {
  if (glob.endsWith("/**")) return file.startsWith(glob.slice(0, -2));
  if (!glob.includes("*")) return file === glob;
  const re = glob
    .split("*")
    .map((part) => part.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
    .join("[^/]*");
  return new RegExp(`^${re}$`).test(file);
}

/** The name of the lane that owns `file`, or null for a shared (foundation-owned) file. */
export function ownerOf(lanes, file) {
  for (const [name, lane] of Object.entries(lanes.lanes)) {
    if (ownedGlobs(lane).some((g) => globMatches(file, g))) return name;
  }
  return null;
}

/** `p` (absolute, or relative to `cwd`) as a repository path with `/`. */
export function repoPath(p, cwd = process.cwd()) {
  return posix(path.relative(ROOT, path.resolve(cwd, p)));
}

/**
 * Shared files whose changes a lane build compiles against: the app's code, its build and tool
 * configuration and the lane table. Docs, CI and these scripts are not.
 */
export function isBuildInput(file) {
  return (
    /^(src|src-tauri|\.cargo)\//.test(file) ||
    /^(dev\/lanes\.json|package\.json|pnpm-lock\.yaml|tsconfig(\.node)?\.json|vite\.config\.ts|vitest\.config\.ts|eslint\.config\.js|index\.html|rust-toolchain\.toml)$/.test(
      file,
    )
  );
}

/** The existing files of a lane under `globs` (repository paths, sorted). */
export function filesOf(globs, root = ROOT) {
  const out = new Set();
  for (const glob of globs) {
    if (glob.endsWith("/**")) {
      const dir = glob.slice(0, -3);
      for (const file of walk(`${root}/${dir}`)) out.add(`${dir}/${file}`);
    } else if (glob.includes("*")) {
      const dir = path.posix.dirname(glob);
      let names = [];
      try {
        names = fs.readdirSync(`${root}/${dir}`);
      } catch {
        // no folder, no files
      }
      for (const name of names) {
        if (globMatches(`${dir}/${name}`, glob)) out.add(`${dir}/${name}`);
      }
    } else if (fs.existsSync(`${root}/${glob}`)) {
      out.add(glob);
    }
  }
  return [...out].sort();
}

/** The files under `dir`, as paths relative to it with `/` (none when it does not exist). */
export function walk(dir) {
  try {
    return fs
      .readdirSync(dir, { recursive: true, withFileTypes: true })
      .filter((e) => e.isFile())
      .map((e) => posix(path.relative(dir, path.join(e.parentPath, e.name))));
  } catch (e) {
    if (e.code === "ENOENT") return [];
    throw e;
  }
}

/** The integration test crates no lane owns (`src-tauri/tests/<name>.rs`), by name. */
export function sharedSuites(lanes, root = ROOT) {
  return fs
    .readdirSync(`${root}/src-tauri/tests`, { withFileTypes: true })
    .filter((e) => e.isFile() && e.name.endsWith(".rs"))
    .map((e) => e.name.slice(0, -3))
    .filter((name) => ownerOf(lanes, `src-tauri/tests/${name}.rs`) === null)
    .sort();
}

/** The folders of a lane's `/**` globs that exist (the renderer folders vitest and eslint take). */
export function existingDirs(globs, root = ROOT) {
  return globs
    .filter((g) => g.endsWith("/**"))
    .map((g) => g.slice(0, -3))
    .filter((d) => fs.existsSync(`${root}/${d}`));
}

// --- Running commands -----------------------------------------------------------------------

const quote = (a) => (/^[\w./:=@,+-]+$/.test(a) ? a : JSON.stringify(a));

/** Runs a command with the terminal as its stdio; resolves to its exit code. */
export function run(cmd, args, { cwd = ROOT, env = process.env, label } = {}) {
  const shown = [cmd === process.execPath ? "node" : cmd, ...args].map(quote).join(" ");
  console.log(`${label ? `[${label}] ` : ""}> ${shown}`);
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd, env, stdio: "inherit" });
    child.on("error", reject);
    child.on("close", (code, signal) => resolve(code ?? (signal ? 1 : 0)));
  });
}

/** Runs a command and collects its output (`input` goes to its stdin). */
export function capture(cmd, args, { cwd = ROOT, env = process.env, input } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
    const out = [];
    const err = [];
    child.stdout.on("data", (d) => out.push(d));
    child.stderr.on("data", (d) => err.push(d));
    child.on("error", reject);
    child.on("close", (code, signal) =>
      resolve({
        code: code ?? (signal ? 1 : 0),
        stdout: Buffer.concat(out).toString("utf8"),
        stderr: Buffer.concat(err).toString("utf8"),
      }),
    );
    child.stdin.on("error", () => {}); // a tool that exits early closes its stdin
    child.stdin.end(input ?? "");
  });
}

/** A node tool of the repository (`node_modules/...`) as a command for `run` / `capture`. */
export function nodeTool(script, args, root = ROOT) {
  return [process.execPath, [`${root}/node_modules/${script}`, ...args]];
}

export const TOOLS = {
  tsc: "typescript/bin/tsc",
  eslint: "eslint/bin/eslint.js",
  prettier: "prettier/bin/prettier.cjs",
  vitest: "vitest/vitest.mjs",
};

/**
 * Splits `files` into argument lists that keep a command line short enough for Windows (32k
 * characters).
 */
export function batches(files, max = 24_000) {
  const out = [];
  let current = [];
  let length = 0;
  for (const f of files) {
    if (current.length && length + f.length + 3 > max) {
      out.push(current);
      current = [];
      length = 0;
    }
    current.push(f);
    length += f.length + 3;
  }
  if (current.length) out.push(current);
  return out;
}

/** `KEY=value` lines of an env file (`#` comments, optional quotes); missing file: none. */
export function readEnvFile(file) {
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch {
    return {};
  }
  const vars = {};
  for (const raw of text.split(/\r?\n/)) {
    const m = /^\s*(?:export\s+)?([A-Za-z_]\w*)\s*=\s*(.*?)\s*$/.exec(raw);
    if (!m || raw.trim().startsWith("#")) continue;
    let value = m[2];
    const q = /^(["'])(.*)\1$/.exec(value);
    if (q) value = q[2];
    else value = value.replace(/\s+#.*$/, "");
    vars[m[1]] = value;
  }
  return vars;
}

// --- Git --------------------------------------------------------------------------------------

/**
 * The working tree's changes (`git status`): `{ x, y, path, from? }` per entry, with untracked
 * files listed one by one.
 */
export async function gitStatus(root = ROOT) {
  const { code, stdout, stderr } = await capture(
    "git",
    ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    { cwd: root },
  );
  if (code !== 0) throw new Error(`git status failed: ${stderr.trim()}`);
  return parseStatus(stdout);
}

export function parseStatus(text) {
  const parts = text.split("\0");
  const out = [];
  for (let i = 0; i < parts.length; i++) {
    const entry = parts[i];
    if (!entry) continue;
    const item = { x: entry[0], y: entry[1], path: entry.slice(3) };
    // A rename or copy: the new path, then the old one.
    if (item.x === "R" || item.x === "C") item.from = parts[++i];
    out.push(item);
  }
  return out;
}

/** The paths a status entry touches (both sides of a rename). */
export const statusPaths = (e) => (e.from ? [e.path, e.from] : [e.path]);

// --- Build slots ------------------------------------------------------------------------------

export const SLOT_ROOT = posix(
  process.env.TABLER_SLOT_ROOT ||
    (process.platform === "win32" ? "C:/t" : path.join(os.homedir(), ".cache", "tabler-slots")),
);
export const LANE_SLOTS = ["s1", "s2", "s3"];
export const GATE_SLOT = "s0";
export const ALL_SLOTS = [GATE_SLOT, ...LANE_SLOTS];
export const TS_SLOTS = ["ts1", "ts2"];

export const slotDir = (slot) => `${SLOT_ROOT}/${slot}`;

/**
 * The environment of a cargo build in `slot`. On Windows openssl-sys links the prebuilt static
 * OpenSSL (as dev/scripts/win-env.sh; its vendored build needs a perl Git Bash lacks), and values
 * already set win. A lane's ts-rs exports go to the slot, never into the repository.
 */
export function cargoEnv(slot, { jobs, lane, laneSlot = true } = {}) {
  const env = { ...process.env };
  if (process.platform === "win32") {
    env.OPENSSL_NO_VENDOR ??= "1";
    env.OPENSSL_STATIC ??= "1";
    env.OPENSSL_DIR ??= "C:/t/openssl";
  }
  env.CARGO_TARGET_DIR = slotDir(slot);
  env.CARGO_BUILD_JOBS = String(jobs);
  if (lane) env.TS_RS_EXPORT_DIR = `${slotDir(slot)}/bindings-${lane}`;
  // Smaller debug info in the lane slots: their test binaries link DuckDB, SQLCipher and aws-lc
  // in 16 GB next to each other. The gates' s0 keeps the profile `cargo test` uses.
  if (laneSlot) env.CARGO_PROFILE_DEV_DEBUG = "line-tables-only";
  return env;
}

/** The cargo arguments that build a lane alone: its features and nothing of the defaults. */
export function laneFeatureArgs(lane, manifest = MANIFEST) {
  return [
    "--manifest-path",
    manifest,
    "--locked",
    "--no-default-features",
    "--features",
    lane.features.join(","),
  ];
}

// --- Locks ----------------------------------------------------------------------------------

/** The lock file of a build slot (in the slot) or a TS slot (in `locks/`). */
export function lockFile(name) {
  return TS_SLOTS.includes(name)
    ? `${SLOT_ROOT}/locks/${name}.lock`
    : `${slotDir(name)}/.tabler-lane.lock`;
}

/** Whether a process with this id runs (a lock whose process is gone is stale). */
export function pidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === "EPERM";
  }
}

/** Who holds the lock in `file`, or null when it is free or stale. */
export function lockHolder(file) {
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (e) {
    if (e.code === "ENOENT") return null;
    throw e;
  }
  try {
    const holder = JSON.parse(text);
    return pidAlive(holder.pid) ? holder : null;
  } catch {
    // Being written right now, or left half-written by a process that died long ago.
    try {
      return Date.now() - fs.statSync(file).mtimeMs < 30_000 ? { pid: 0, what: "starting" } : null;
    } catch {
      return null;
    }
  }
}

const held = new Set();

/** Takes the lock `name` if it is free (a stale one is removed); returns null or its holder. */
export function tryLock(name, what) {
  const file = lockFile(name);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const fd = fs.openSync(file, "wx");
      const since = new Date().toISOString();
      fs.writeSync(fd, JSON.stringify({ pid: process.pid, what, since }));
      fs.closeSync(fd);
      held.add(name);
      return null;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
    }
    const text = readOr(file, "");
    const holder = lockHolder(file);
    if (holder) return holder;
    // Stale: remove it, unless someone else replaced it in the meantime.
    if (readOr(file, "") === text) {
      try {
        fs.unlinkSync(file);
      } catch (e) {
        if (e.code !== "ENOENT") throw e;
      }
    }
  }
  return lockHolder(file) ?? { pid: 0, what: "contended" };
}

function readOr(file, fallback) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return fallback;
  }
}

/** Releases a lock this process holds. */
export function release(name) {
  const file = lockFile(name);
  try {
    if (JSON.parse(fs.readFileSync(file, "utf8")).pid === process.pid) fs.unlinkSync(file);
  } catch {
    // already gone
  }
  held.delete(name);
}

export function releaseAll() {
  for (const name of [...held]) release(name);
}

process.on("exit", releaseAll);
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", "SIGBREAK"]) {
  process.on(signal, () => {
    releaseAll();
    process.exit(130);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const describe = (h) => `${h.what ?? "?"}${h.pid ? `, pid ${h.pid}` : ""}`;

/**
 * Waits until one of `names` is free and takes it, trying them in order (the first is the
 * preferred one); returns the name taken.
 */
export async function acquireAny(names, what, { pollMs = 2000, log = console.error } = {}) {
  let last = "";
  for (;;) {
    const busy = [];
    for (const name of names) {
      const holder = tryLock(name, what);
      if (!holder) return name;
      busy.push(`${name}: ${describe(holder)}`);
    }
    const message = busy.join("; ");
    if (message !== last) log(`[lanes] waiting for ${names.join(" or ")} (${message})`);
    last = message;
    await sleep(pollMs);
  }
}

/** Takes every lock of `names`, in the order given (always the same order: no deadlock). */
export async function acquireAll(names, what, opts) {
  for (const name of names) await acquireAny([name], what, opts);
  return names;
}

// --- HEAD snapshot ----------------------------------------------------------------------------

/**
 * A copy of HEAD in `<slot root>/snap/<name>` with `overlay` (repository paths) taken from the
 * working tree, or removed where the working tree deleted them: what a build sees once those
 * paths are committed, whatever else is uncommitted. `git archive`, not a worktree.
 */
export async function snapshot(name, overlay, { nodeModules = false } = {}) {
  const dir = `${SLOT_ROOT}/snap/${name}`;
  unlinkNodeModules(dir);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  // tar runs inside the folder: GNU tar reads `-C C:/…` as a remote host.
  await new Promise((resolve, reject) => {
    const git = spawn("git", ["archive", "--format=tar", "HEAD"], {
      cwd: ROOT,
      stdio: ["ignore", "pipe", "inherit"],
    });
    const tar = spawn("tar", ["-xf", "-"], { cwd: dir, stdio: ["pipe", "inherit", "inherit"] });
    git.stdout.pipe(tar.stdin);
    let pending = 2;
    const done = (what) => (code) => {
      if (code !== 0) reject(new Error(`${what} exited with ${code}`));
      else if (--pending === 0) resolve();
    };
    git.on("error", reject).on("close", done("git archive"));
    tar.on("error", reject).on("close", done("tar"));
  });
  for (const file of overlay) {
    const from = `${ROOT}/${file}`;
    const to = `${dir}/${file}`;
    if (fs.existsSync(from)) {
      fs.mkdirSync(path.dirname(to), { recursive: true });
      fs.copyFileSync(from, to);
    } else {
      fs.rmSync(to, { force: true });
    }
  }
  // Every source newer than any earlier build of this folder: git archive dates the files at the
  // commit and a copy may keep its own date, and cargo would take an older file for unchanged.
  const now = new Date();
  for (const file of walk(`${dir}/src-tauri`)) fs.utimesSync(`${dir}/src-tauri/${file}`, now, now);
  if (nodeModules) {
    // The renderer tools resolve packages from the repository's node_modules.
    fs.symlinkSync(`${ROOT}/node_modules`, `${dir}/node_modules`, "junction");
  }
  return dir;
}

/** Removes a snapshot's node_modules link (never what it points to). */
function unlinkNodeModules(dir) {
  const link = `${dir}/node_modules`;
  let stat;
  try {
    stat = fs.lstatSync(link);
  } catch {
    return;
  }
  if (!stat.isSymbolicLink()) {
    throw new Error(`${link} is not a link to the repository's node_modules; remove it by hand`);
  }
  fs.unlinkSync(link);
}

// --- rustfmt ----------------------------------------------------------------------------------

/**
 * Formats each file on its own through rustfmt's stdin, so that a `mod.rs` never pulls in the
 * modules it declares (a lane's unfinished files, or the shared ones around a lane's). Returns
 * the files that are not formatted (`write`: that were rewritten) and those rustfmt rejected.
 */
export async function rustfmtFiles(files, { write = false, root = ROOT, concurrency = 6 } = {}) {
  const changed = [];
  const failed = [];
  const queue = [...files];
  async function worker() {
    for (let file = queue.shift(); file !== undefined; file = queue.shift()) {
      const text = fs.readFileSync(`${root}/${file}`, "utf8");
      const { code, stdout, stderr } = await capture(
        "rustfmt",
        ["--edition", "2024", "--emit", "stdout"],
        { cwd: `${root}/src-tauri`, input: text },
      );
      if (code !== 0) {
        failed.push({ file, message: stderr.trim() });
        continue;
      }
      const lf = (s) => s.replace(/\r\n/g, "\n");
      if (lf(stdout) !== lf(text)) {
        changed.push(file);
        // rustfmt answers stdin with LF; the file keeps the line endings it has.
        const crlf = /^[^\n]*\r\n/.test(text);
        if (write)
          fs.writeFileSync(
            `${root}/${file}`,
            crlf ? lf(stdout).replace(/\n/g, "\r\n") : lf(stdout),
          );
      }
    }
  }
  await Promise.all(Array.from({ length: concurrency }, worker));
  return { changed: changed.sort(), failed };
}

// --- tsc --------------------------------------------------------------------------------------

/**
 * The diagnostics of `tsc --pretty false` output: `{ file, line, code, text }`, with `file`
 * null for one that names no file (a config error).
 */
export function parseTsc(output) {
  const out = [];
  for (const line of output.split(/\r?\n/)) {
    const m = /^(.+?)\((\d+),(\d+)\): (?:error|warning) (TS\d+): (.*)$/.exec(line);
    if (m) {
      out.push({ file: posix(m[1]), line: Number(m[2]), code: m[4], text: line });
    } else if (/^(?:error|warning) TS\d+:/.test(line)) {
      out.push({ file: null, line: 0, code: "", text: line });
    } else if (line.startsWith(" ") && out.length) {
      out[out.length - 1].text += `\n${line}`;
    }
  }
  return out;
}

/**
 * Runs tsc over the project in `root` and splits its diagnostics: `failing` are those in shared
 * files and, for a lane, in its own files; `ignored` counts the other lanes' by lane. `lane` null
 * is the foundation's mode, which ignores every lane's files.
 */
export async function tscCheck(lanes, lane, root = ROOT) {
  const [cmd, args] = nodeTool(TOOLS.tsc, ["--noEmit", "--pretty", "false"], root);
  const { code, stdout, stderr } = await capture(cmd, args, { cwd: root });
  const diagnostics = parseTsc(stdout + stderr);
  const failing = [];
  const ignored = {};
  for (const d of diagnostics) {
    const owner = d.file === null ? null : ownerOf(lanes, d.file);
    if (owner === null || owner === lane) failing.push(d);
    else ignored[owner] = (ignored[owner] ?? 0) + 1;
  }
  if (code !== 0 && diagnostics.length === 0) {
    failing.push({ file: null, line: 0, code: "", text: (stdout + stderr).trim() });
  }
  return { failing, ignored };
}
