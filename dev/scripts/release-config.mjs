// dev/scripts/release-config.mjs -- the version and updater settings of a CI build
// (.github/workflows/build.yml, docs/updates.md):
//
//   node dev/scripts/release-config.mjs --version
//       print the version of the build of this commit
//   node dev/scripts/release-config.mjs --apply --build-version <v> --out <file>
//       write <v> into package.json, src-tauri/Cargo.toml and src-tauri/tauri.conf.json (in the
//       CI checkout only, never committed) and the Tauri config to merge over tauri.conf.json
//
// The version. Nobody bumps it by hand: every published build is a `v*` git tag, and the next
// build takes the next patch number.
//   - a `v*` tag pushed by hand builds that version (`v0.2.0` -> `0.2.0`): a numbered release,
//     the stable channel;
//   - a commit that already carries a `v*` tag builds that version again (a re-run);
//   - anything else builds the highest version so far with its patch number raised:
//     `0.1.7` after `v0.1.6`. "So far" is the highest `v*` tag, or the version in
//     tauri.conf.json when that is higher (a repository without tags starts at `0.1.1`).
//     The publish job then tags the commit `v0.1.7`.
//
// The updater: a build made with the signing key (`TABLER_UPDATER_SIGNED=true`) produces updater
// artifacts and follows its channel's `latest.json` on this repository's releases. A build made
// without it (a fork) produces none and gets no update address, so it never follows releases it
// cannot verify or publish.
//
// Reads GITHUB_REF, GITHUB_REPOSITORY and TABLER_UPDATER_SIGNED; `--ref`, `--repo`, `--signed`,
// `--tags` and `--head-tags` (comma separated) override them and what git says, for the tests.

import fs from "node:fs";
import { fileURLToPath } from "node:url";

import {
  applyBuildVersion,
  isPlain,
  parseVersion,
  versionForCheckout,
  wixVersion,
} from "./build-version.mjs";
export {
  buildVersion,
  compareVersions,
  isPlain,
  parseVersion,
  plainVersions,
  tagVersion,
  VERSION_FILES,
  versionIn,
  withVersion,
  wixVersion,
} from "./build-version.mjs";

/**
 * The channel of a build: `stable` for a numbered release (a `v*` tag pushed by hand), `nightly`
 * for the automatic build of a commit, null for a pre-release tag (`v1.0.0-rc.1`), which belongs
 * to neither.
 */
export function channelOf(ref, version) {
  if (!(ref ?? "").startsWith("refs/tags/v")) return "nightly";
  return isPlain(version) ? "stable" : null;
}

/** The `latest.json` a channel's builds ask (`repo`: `owner/name` on GitHub). */
export function endpointOf(channel, repo) {
  if (channel === "stable")
    return `https://github.com/${repo}/releases/latest/download/latest.json`;
  if (channel === "nightly")
    return `https://github.com/${repo}/releases/download/nightly/latest.json`;
  return null;
}

/** The config merged over tauri.conf.json for the build of `version`. */
export function releaseConfig({ ref, version, repo, signed }) {
  if (!parseVersion(version)) throw new Error(`not a version: ${version}`);
  const endpoint = signed && repo ? endpointOf(channelOf(ref, version), repo) : null;
  const config = {
    version,
    bundle: { createUpdaterArtifacts: Boolean(signed) },
    plugins: { updater: { endpoints: endpoint ? [endpoint] : [] } },
  };
  const wix = wixVersion(version);
  if (wix) config.bundle.windows = { wix: { version: wix } };
  return config;
}

function main(argv) {
  const arg = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const list = (name) =>
    arg(name) === undefined ? undefined : arg(name).split(",").filter(Boolean);
  const ref = arg("--ref") ?? process.env.GITHUB_REF ?? "";
  const repo = arg("--repo") ?? process.env.GITHUB_REPOSITORY ?? "";
  const signed = (arg("--signed") ?? process.env.TABLER_UPDATER_SIGNED ?? "") === "true";
  const version =
    arg("--build-version") ??
    versionForCheckout({
      ref,
      tags: list("--tags"),
      headTags: list("--head-tags"),
    });
  const config = releaseConfig({ ref, version, repo, signed });
  if (argv.includes("--version")) {
    console.log(version);
    if (process.env.GITHUB_OUTPUT)
      fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\n`);
    return;
  }
  const out = arg("--out");
  if (!argv.includes("--apply") || !out)
    throw new Error(
      "usage: release-config.mjs --version | --apply --build-version <v> --out <file>",
    );
  applyBuildVersion(version);
  fs.writeFileSync(out, `${JSON.stringify(config, null, 2)}\n`);
  const endpoint = config.plugins.updater.endpoints[0];
  console.log(
    endpoint
      ? `version ${version}; updater artifacts on, updates from ${endpoint}`
      : `version ${version}; no updater artifacts and no update address (${signed ? "a pre-release tag has no channel" : "no signing key"})`,
  );
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
    console.error(`release-config: ${e.message}`);
    process.exit(1);
  }
}
