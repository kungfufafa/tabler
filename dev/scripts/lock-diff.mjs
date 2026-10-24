#!/usr/bin/env node
// dev/scripts/lock-diff.mjs -- checks that a change to src-tauri/Cargo.lock only adds packages.
//
//   node dev/scripts/lock-diff.mjs                       # HEAD's lock against the working tree's
//   node dev/scripts/lock-diff.mjs --base <rev>          # another commit's lock
//   node dev/scripts/lock-diff.mjs --old <file> --new <file> [--list]
//
// New dependencies are added by resolving (`cargo metadata`), never `cargo update`, so every
// package of the old lock must still be there with the same version and checksum. What may change
// around them:
// - an existing package gains dependencies: a new crate turned on one of its optional ones;
// - a dependency that allows several versions moves to another version the old lock already had:
//   cargo unlocks the dependency edges (not the versions) whenever a manifest adds a registry
//   crate, then takes the highest locked version in range. These are listed for review.
// Anything else (a package gone, a new version, a changed checksum, an edge moved to a version
// the old lock lacked) fails with exit code 1.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const LOCK = "src-tauri/Cargo.lock";

function args(argv) {
  const out = { base: "HEAD", old: null, new: null, list: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--list") out.list = true;
    else if (a === "--base" || a === "--old" || a === "--new") {
      const value = argv[++i];
      if (!value) throw new Error(`${a} needs a value`);
      out[a.slice(2)] = value;
    } else throw new Error(`unknown argument ${a}`);
  }
  return out;
}

/** The `[[package]]` entries of a Cargo.lock (format 3 or 4). */
function parseLock(text) {
  const packages = [];
  let current = null;
  let inDeps = false;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (inDeps) {
      if (line === "]") inDeps = false;
      else current.dependencies.push(JSON.parse(line.replace(/,$/, "")));
      continue;
    }
    if (line === "[[package]]") {
      current = { name: "", version: "", source: null, checksum: null, dependencies: [] };
      packages.push(current);
    } else if (line.startsWith("[")) {
      // `[metadata]`, `[[patch.unused]]` and the like end the package list.
      current = null;
    } else if (current) {
      const m = /^(\w+) = (.*)$/.exec(line);
      if (!m) continue;
      const [, key, value] = m;
      if (key === "dependencies") {
        if (value === "[") inDeps = true;
        else current.dependencies = JSON.parse(value);
      } else if (key in current) {
        current[key] = JSON.parse(value);
      }
    }
  }
  return packages;
}

const id = (p) => `${p.name} ${p.version}${p.source ? ` (${p.source})` : ""}`;
const short = (p) => `${p.name} ${p.version}`;

/** Resolves a lock dependency entry (`name`, `name version` or `name version (source)`). */
function resolver(packages) {
  const byName = new Map();
  for (const p of packages) byName.set(p.name, [...(byName.get(p.name) ?? []), p]);
  return (dep) => {
    const m = /^(\S+)(?: (\S+))?(?: \((.+)\))?$/.exec(dep);
    const hits = m
      ? (byName.get(m[1]) ?? []).filter(
          (p) => (!m[2] || p.version === m[2]) && (!m[3] || p.source === m[3]),
        )
      : [];
    if (hits.length !== 1) throw new Error(`cannot resolve lock dependency "${dep}"`);
    return hits[0];
  };
}

function compare(oldLock, newLock) {
  const oldPkgs = parseLock(oldLock);
  const newPkgs = parseLock(newLock);
  const oldDep = resolver(oldPkgs);
  const newDep = resolver(newPkgs);
  const oldIds = new Set(oldPkgs.map(id));
  const newById = new Map(newPkgs.map((p) => [id(p), p]));
  const problems = [];
  const repointed = [];
  const gained = [];
  for (const p of oldPkgs) {
    const n = newById.get(id(p));
    if (!n) {
      const now = newPkgs.filter((q) => q.name === p.name).map((q) => q.version);
      problems.push(`${short(p)}: gone${now.length ? ` (now ${now.join(", ")})` : ""}`);
      continue;
    }
    if (p.checksum !== n.checksum) problems.push(`${short(p)}: checksum changed`);
    const before = p.dependencies.map(oldDep);
    const after = n.dependencies.map(newDep);
    const afterIds = new Set(after.map(id));
    const beforeIds = new Set(before.map(id));
    for (const dep of before.filter((d) => !afterIds.has(id(d)))) {
      const now = after.filter((d) => d.name === dep.name && !beforeIds.has(id(d)));
      const moved = `${short(p)}: ${short(dep)} -> ${now.map((d) => d.version).join(", ") || "none"}`;
      if (now.length === 1 && oldIds.has(id(now[0]))) repointed.push(moved);
      else problems.push(moved);
    }
    for (const dep of after.filter((d) => !beforeIds.has(id(d)))) {
      if (!before.some((d) => d.name === dep.name)) gained.push(`${short(p)}: + ${short(dep)}`);
    }
  }
  const added = newPkgs.filter((p) => !oldIds.has(id(p))).map(short);
  return { problems, repointed, gained, added };
}

function readLock(opts, which) {
  if (opts[which]) return readFileSync(opts[which], "utf8");
  if (which === "new") return readFileSync(`${root}/${LOCK}`, "utf8");
  return execFileSync("git", ["show", `${opts.base}:${LOCK}`], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

const opts = args(process.argv.slice(2));
const { problems, repointed, gained, added } = compare(
  readLock(opts, "old"),
  readLock(opts, "new"),
);
console.log(
  `[lock-diff] ${added.length} package(s) added, ${gained.length} optional dependency edge(s) ` +
    `turned on, ${repointed.length} edge(s) moved between locked versions`,
);
if (opts.list) {
  for (const a of added) console.log(`  + ${a}`);
  for (const g of gained) console.log(`  ~ ${g}`);
}
for (const r of repointed) console.log(`  > ${r}`);
if (problems.length) {
  console.error(`[lock-diff] ${problems.length} change(s) to existing packages:`);
  for (const p of problems) console.error(`  ! ${p}`);
  process.exit(1);
}
console.log("[lock-diff] ok: no existing package changed version or checksum");
