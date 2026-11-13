// Choose one installer version for the workflow, then apply it in each platform's checkout.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

/** The three files that state the app's version; they always hold the same one. */
export const VERSION_FILES = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml"];

/** MAJOR.MINOR.PATCH with an optional pre-release; build metadata is not used here. */
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?$/;

/** The parts of a version, or null when it is not one this script builds. */
export function parseVersion(version) {
  const m = SEMVER.exec(version ?? "");
  if (!m) return null;
  return { major: +m[1], minor: +m[2], patch: +m[3], pre: m[4] ?? "" };
}

/** Whether `version` is a plain MAJOR.MINOR.PATCH. */
export function isPlain(version) {
  const parsed = parseVersion(version);
  return parsed !== null && parsed.pre === "";
}

/** Orders two plain versions: negative when `a` is the lower one. */
export function compareVersions(a, b) {
  const [pa, pb] = [parseVersion(a), parseVersion(b)];
  return pa.major - pb.major || pa.minor - pb.minor || pa.patch - pb.patch;
}

/** `0.2.0` of `refs/tags/v0.2.0` or of the tag name `v0.2.0`; null for anything else. */
export function tagVersion(ref) {
  const m = /^(?:refs\/tags\/)?v(.+)$/.exec(ref ?? "");
  return m ? m[1] : null;
}

/** The plain versions among tag names (`v0.1.6`, `nightly`, `v1.0.0-rc.1` -> `0.1.6`), highest first. */
export function plainVersions(tags) {
  return tags
    .map(tagVersion)
    .filter((v) => v !== null && isPlain(v))
    .sort(compareVersions)
    .reverse();
}

/**
 * The version a build of `ref` gets. `base`: the version in tauri.conf.json; `tags`: every tag of
 * the repository; `headTags`: the tags on the commit built.
 */
export function buildVersion({ ref, base, tags = [], headTags = [] }) {
  if ((ref ?? "").startsWith("refs/tags/v")) {
    const tag = tagVersion(ref);
    if (!parseVersion(tag))
      throw new Error(`the tag v${tag} is not a version (vMAJOR.MINOR.PATCH)`);
    return tag;
  }
  if (!isPlain(base))
    throw new Error(`the version in tauri.conf.json is not MAJOR.MINOR.PATCH: ${base}`);
  // Keep the version already assigned to this commit.
  const [published] = plainVersions(headTags);
  if (published) return published;
  const [latest] = plainVersions(tags);
  const from = latest && compareVersions(latest, base) > 0 ? latest : base;
  const { major, minor, patch } = parseVersion(from);
  return `${major}.${minor}.${patch + 1}`;
}

/**
 * The MSI ProductVersion of a pre-release tag (WiX takes numbers only): `1.0.0` for
 * `1.0.0-rc.1`; null for a plain version, which is its own.
 */
export function wixVersion(version) {
  const parsed = parseVersion(version);
  if (!parsed || !parsed.pre) return null;
  return `${parsed.major}.${parsed.minor}.${parsed.patch}`;
}

/** `text` (one of the three version files, by `file`'s name) with its version set to `version`. */
export function withVersion(file, text, version) {
  if (file.endsWith("Cargo.toml")) {
    // The first `version = "…"` is the package's: [package] opens the manifest.
    if (!/^\[package\]/m.test(text.split(/^version\s*=/m)[0] ?? ""))
      throw new Error(`${file}: no version under [package]`);
    return text.replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`);
  }
  // The first `"version"` key is the top-level one in both JSON files.
  if (!/^\s*"version"\s*:\s*"[^"]*"/m.test(text)) throw new Error(`${file}: no "version"`);
  return text.replace(/^(\s*"version"\s*:\s*)"[^"]*"/m, `$1"${version}"`);
}

/** The version `text` (one of the three version files) states. */
export function versionIn(file, text) {
  const m = file.endsWith("Cargo.toml")
    ? /^version\s*=\s*"([^"]*)"/m.exec(text)
    : /^\s*"version"\s*:\s*"([^"]*)"/m.exec(text);
  if (!m) throw new Error(`${file}: no version`);
  return m[1];
}

const CARGO_LOCK = "src-tauri/Cargo.lock";
const TABLER_LOCK_VERSION = /(\[\[package\]\]\r?\nname = "tabler"\r?\nversion = )"[^"]*"/;

const read = (file) => fs.readFileSync(path.join(ROOT, file), "utf8");
const git = (...args) => execFileSync("git", args, { cwd: ROOT, encoding: "utf8" }).trim();
const versionTags = (...args) =>
  git("tag", "--list", "v*", ...args)
    .split("\n")
    .filter(Boolean);

export function versionForCheckout({ ref, tags, headTags }) {
  if (!tags && git("rev-parse", "--is-shallow-repository") === "true")
    throw new Error("the checkout is shallow: the version needs the tags (fetch-depth: 0)");
  return buildVersion({
    ref,
    base: versionIn("src-tauri/tauri.conf.json", read("src-tauri/tauri.conf.json")),
    tags: tags ?? versionTags(),
    headTags: headTags ?? versionTags("--points-at", "HEAD"),
  });
}

export function applyBuildVersion(version) {
  if (!parseVersion(version)) throw new Error(`not a version: ${version}`);
  const lock = read(CARGO_LOCK);
  if (!TABLER_LOCK_VERSION.test(lock)) throw new Error(`${CARGO_LOCK}: no Tabler package version`);
  const updates = VERSION_FILES.map((file) => [file, withVersion(file, read(file), version)]);
  updates.push([CARGO_LOCK, lock.replace(TABLER_LOCK_VERSION, `$1"${version}"`)]);
  for (const [file, content] of updates) fs.writeFileSync(path.join(ROOT, file), content);
}

/** Reserve the version after the installers and CI pass. Skip runs older than the live nightly. */
function tagNightly(version) {
  if (!isPlain(version)) throw new Error(`invalid nightly version: ${version}`);
  git("fetch", "--force", "--prune", "--prune-tags", "--tags", "origin");
  const head = git("rev-parse", "HEAD");
  if (git("tag", "--list", "nightly")) {
    const previous = git("rev-parse", "refs/tags/nightly^{commit}");
    if (previous !== head) {
      const [latest] = plainVersions(versionTags("--points-at", previous));
      const ancestry = spawnSync("git", ["merge-base", "--is-ancestor", head, previous], {
        cwd: ROOT,
      });
      if (ancestry.status !== 0 && ancestry.status !== 1)
        throw new Error("could not compare the build with the current nightly");
      if (ancestry.status === 0 || (latest && compareVersions(version, latest) <= 0)) {
        console.log(
          `Skipping ${head.slice(0, 7)}. Nightly already has a newer build at ${previous.slice(0, 7)}.`,
        );
        return false;
      }
    }
  }
  const expected = versionForCheckout({ ref: "refs/heads/main" });
  if (version !== expected)
    throw new Error(
      `built ${version}, but the next version is ${expected}. Rerun all jobs to rebuild.`,
    );

  const tag = `v${version}`;
  if (!git("tag", "--list", tag)) git("push", "origin", `${head}:refs/tags/${tag}`);
  return true;
}

function main([command, version, out]) {
  const output = (name, value) => {
    if (process.env.GITHUB_OUTPUT)
      fs.appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  };
  if (command === "--version") {
    const chosen = versionForCheckout({ ref: process.env.GITHUB_REF });
    console.log(chosen);
    output("version", chosen);
  } else if (command === "--apply" && version && out) {
    applyBuildVersion(version);
    const config = { version };
    const wix = wixVersion(version);
    if (wix) config.bundle = { windows: { wix: { version: wix } } };
    fs.writeFileSync(out, `${JSON.stringify(config, null, 2)}\n`);
  } else if (command === "--tag-nightly" && version) {
    output("publish", tagNightly(version));
  } else {
    throw new Error(
      "usage: build-version.mjs --version | --apply <version> <config> | --tag-nightly <version>",
    );
  }
}

if (
  process.argv[1] &&
  fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`build-version: ${error.message}`);
    process.exitCode = 1;
  }
}
