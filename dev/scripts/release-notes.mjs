// dev/scripts/release-notes.mjs -- what changed in a version, from the commits
// (.github/workflows/build.yml, docs/updates.md):
//
//   node dev/scripts/release-notes.mjs --version <v> [--format text|markdown] [--versions 8]
//
// Every published build is a `v*` tag, so a version's changes are the commits between its tag
// and the one before. The notes list the version being published (`HEAD`) and the versions
// before it, newest first, each with its features ("New"), fixes ("Fixed") and other changes a
// user notices; commits about CI, tests, docs and housekeeping are left out.
//   - `text` is what the update dialog shows (latest.json's `notes`);
//   - `markdown` is the body of the GitHub release.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { compareVersions, isPlain, plainVersions } from "./release-config.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

/** The groups of a version's notes, in the order shown. */
export const GROUPS = ["New", "Fixed", "Improved", "Changed"];

/** Commit types (`type: subject`) a user of the app does not notice. */
const HIDDEN = new Set([
  "ci",
  "test",
  "tests",
  "docs",
  "doc",
  "chore",
  "build",
  "style",
  "dev",
  "release",
]);
const GROUP_OF = {
  feat: "New",
  feature: "New",
  fix: "Fixed",
  perf: "Improved",
  refactor: "Improved",
  ui: "Improved",
};

/** How many changes one group lists before "… and N more". */
export const MAX_ITEMS = 12;

/**
 * GitHub ends the subject of a squash-merged pull request with its number: `fix: … (#14)`.
 * `{ line, pr }`: the subject without it, and the number (null for a commit that has none).
 */
function splitPullRequest(subject) {
  const line = (subject ?? "").trim();
  const m = /\s+\(#(\d+)\)$/.exec(line);
  return m ? { line: line.slice(0, m.index), pr: Number(m[1]) } : { line, pr: null };
}

/**
 * Where a commit subject goes: `{ group, text }`, or null when it is not for the notes.
 * `feat(grid): sort by click (#14)` -> New "Sort by click"; a subject without a type is "Changed".
 */
export function classify(subject) {
  const { line } = splitPullRequest(subject);
  if (!line || /^(Merge|Revert) /.test(line)) return null;
  const m = /^([A-Za-z]+)(?:\([^)]*\))?!?:\s*(.+)$/.exec(line);
  const type = m ? m[1].toLowerCase() : null;
  if (type && HIDDEN.has(type)) return null;
  const group = type ? GROUP_OF[type] : undefined;
  // A type this script does not know (`redis: …`, `icon: …`) says what the change is about: it
  // stays in the text.
  const text = (group ? m[2] : line).trim();
  return { group: group ?? "Changed", text: text[0].toUpperCase() + text.slice(1) };
}

/**
 * A version's commit subjects (newest first) as groups: `[{ group, items, more }]`.
 * `pullRequests` keeps each change's pull request number (`Sort by click (#14)`), which GitHub
 * turns into a link on the release page; the update dialog shows plain text and leaves it out.
 */
export function groupChanges(subjects, { pullRequests = false } = {}) {
  // Per group: a change's text -> its pull request. The same change twice is listed once.
  const byGroup = new Map(GROUPS.map((g) => [g, new Map()]));
  for (const subject of subjects) {
    const change = classify(subject);
    if (change && !byGroup.get(change.group).has(change.text))
      byGroup.get(change.group).set(change.text, splitPullRequest(subject).pr);
  }
  return GROUPS.filter((g) => byGroup.get(g).size > 0).map((group) => {
    const items = [...byGroup.get(group)].map(([text, pr]) =>
      pullRequests && pr ? `${text} (#${pr})` : text,
    );
    return { group, items: items.slice(0, MAX_ITEMS), more: Math.max(0, items.length - MAX_ITEMS) };
  });
}

/** What a version with nothing for the notes says. */
export const NO_CHANGES = "Maintenance build: nothing changed in the app itself.";

/**
 * The notes of `sections` (`[{ version, subjects }]`, newest first). `text`: a version on a line
 * of its own, then its groups; `markdown`: the same as headings and lists.
 */
export function renderNotes(sections, format = "text") {
  const md = format === "markdown";
  const out = [];
  for (const { version, subjects } of sections) {
    out.push(md ? `## ${version}` : version, "");
    const groups = groupChanges(subjects, { pullRequests: md });
    if (groups.length === 0) out.push(NO_CHANGES, "");
    for (const { group, items, more } of groups) {
      out.push(md ? `**${group}**` : group, ...(md ? [""] : []));
      out.push(...items.map((item) => `- ${item}`));
      if (more > 0) out.push(md ? `- … and ${more} more` : `- … and ${more} more`);
      out.push("");
    }
  }
  return `${out.join("\n").trim()}\n`;
}

/**
 * The commit ranges of the newest versions: the one being published (`version`, up to `head`),
 * then the tagged ones before it. `tags`: every tag name. A range's `from` is the tag of the
 * version before it; the first version ever has none (`null`: its whole history).
 */
export function versionRanges({ version, head, tags, count }) {
  const older = plainVersions(tags).filter(
    (v) => !isPlain(version) || compareVersions(v, version) < 0,
  );
  const chain = [{ version, to: head }, ...older.map((v) => ({ version: v, to: `v${v}` }))];
  return chain.slice(0, count).map((entry, i) => ({ ...entry, from: chain[i + 1]?.to ?? null }));
}

const git = (...args) => execFileSync("git", args, { cwd: ROOT, encoding: "utf8" }).trim();

function main(argv) {
  const arg = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const version = arg("--version");
  if (!version) throw new Error("usage: release-notes.mjs --version <v> [--format text|markdown]");
  const format = arg("--format") ?? "text";
  const count = Number(arg("--versions") ?? 8);
  const tags = git("tag", "--list", "v*").split("\n").filter(Boolean);
  const ranges = versionRanges({ version, head: arg("--head") ?? "HEAD", tags, count });
  const sections = ranges.map(({ version: v, from, to }) => ({
    version: v,
    // The first version ever would list the whole history: its newest commits stand for it.
    subjects: git(
      "log",
      "--no-merges",
      "--format=%s",
      ...(from ? [`${from}..${to}`] : ["-n", "30", to]),
    )
      .split("\n")
      .filter(Boolean),
  }));
  process.stdout.write(renderNotes(sections, format));
}

/** Whether this file is the script node was started with (not imported by a test). */
function isMain() {
  if (!process.argv[1]) return false;
  // Real paths: a temp folder behind a symlink, or a drive letter in another case, is the same file.
  const same = (p) => (process.platform === "win32" ? p.toLowerCase() : p);
  try {
    return (
      same(fs.realpathSync(fileURLToPath(import.meta.url))) ===
      same(fs.realpathSync(process.argv[1]))
    );
  } catch {
    return false;
  }
}

if (isMain()) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`release-notes: ${e.message}`);
    process.exit(1);
  }
}
