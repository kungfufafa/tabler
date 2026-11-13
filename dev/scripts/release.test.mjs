// dev/scripts/release.test.mjs -- tests of the release scripts: node --test dev/scripts/release.test.mjs

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import {
  buildVersion,
  channelOf,
  compareVersions,
  endpointOf,
  isPlain,
  parseVersion,
  plainVersions,
  releaseConfig,
  tagVersion,
  VERSION_FILES,
  versionIn,
  withVersion,
  wixVersion,
} from "./release-config.mjs";
import {
  classify,
  groupChanges,
  MAX_ITEMS,
  NO_CHANGES,
  renderNotes,
  versionRanges,
} from "./release-notes.mjs";
import { buildManifest, signedVersion, targetsOf } from "./updater-manifest.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.join(HERE, "../..");
const TMP = fs.mkdtempSync(path.join(os.tmpdir(), "tabler-release-"));
after(() => fs.rmSync(TMP, { recursive: true, force: true }));

/** A `.sig` as `tauri build` writes it: base64 of the minisign text, the version in its comment. */
function sig(file, version) {
  const comment = ["timestamp:1794470400", `file:${file}`, version && `version:${version}`]
    .filter(Boolean)
    .join("\t");
  const text = `untrusted comment: signature from tauri secret key\nRUTest\ntrusted comment: ${comment}\nGlobalSig\n`;
  return Buffer.from(text).toString("base64");
}

describe("the version of a build", () => {
  const main = "refs/heads/main";

  it("reads versions and tags", () => {
    assert.deepEqual(parseVersion("0.1.7"), { major: 0, minor: 1, patch: 7, pre: "" });
    assert.equal(parseVersion("0.1"), null);
    assert.equal(parseVersion(undefined), null);
    assert.ok(isPlain("0.1.7"));
    assert.ok(!isPlain("1.0.0-rc.1"));
    assert.ok(!isPlain("nightly"));
    assert.equal(tagVersion("refs/tags/v0.2.0"), "0.2.0");
    assert.equal(tagVersion("v0.2.0"), "0.2.0");
    assert.equal(tagVersion("refs/heads/main"), null);
    assert.equal(tagVersion("nightly"), null);
    // Numbers, not text: 0.1.10 is after 0.1.9.
    assert.ok(compareVersions("0.1.9", "0.1.10") < 0);
    assert.ok(compareVersions("0.2.0", "0.1.99") > 0);
    assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
    assert.deepEqual(
      plainVersions(["v0.1.9", "nightly", "v0.1.10", "v1.0.0-rc.1", "v0.1.2", "vnext"]),
      ["0.1.10", "0.1.9", "0.1.2"],
    );
  });

  it("starts at 0.1.1 in a repository without version tags", () => {
    assert.equal(
      buildVersion({ ref: main, base: "0.1.0", tags: ["nightly"], headTags: [] }),
      "0.1.1",
    );
    assert.equal(buildVersion({ ref: main, base: "0.1.0" }), "0.1.1");
  });

  it("raises the patch number of the newest version, one build after the other", () => {
    const tags = ["nightly"];
    const published = [];
    // Each published build tags its commit; the next build counts from that tag.
    for (let i = 0; i < 12; i++) {
      const version = buildVersion({ ref: main, base: "0.1.0", tags, headTags: [] });
      published.push(version);
      tags.push(`v${version}`);
    }
    assert.deepEqual(published.slice(0, 3), ["0.1.1", "0.1.2", "0.1.3"]);
    assert.equal(published.at(-1), "0.1.12");
    for (let i = 1; i < published.length; i++)
      assert.ok(compareVersions(published[i - 1], published[i]) < 0, published[i]);
  });

  it("does not use a number twice when a build was never published", () => {
    // A cancelled or failed build leaves no tag: the next build takes the same number.
    const tags = ["v0.1.1", "v0.1.2"];
    assert.equal(buildVersion({ ref: main, base: "0.1.0", tags, headTags: [] }), "0.1.3");
    assert.equal(buildVersion({ ref: main, base: "0.1.0", tags, headTags: [] }), "0.1.3");
  });

  it("builds a commit that was already published as the same version", () => {
    const tags = ["v0.1.1", "v0.1.2", "v0.1.3"];
    assert.equal(
      buildVersion({ ref: main, base: "0.1.0", tags, headTags: ["v0.1.3", "nightly"] }),
      "0.1.3",
    );
    assert.equal(buildVersion({ ref: main, base: "0.1.0", tags, headTags: ["nightly"] }), "0.1.4");
  });

  it("continues after a release tagged by hand, or a higher version in tauri.conf.json", () => {
    assert.equal(buildVersion({ ref: main, base: "0.1.0", tags: ["v0.1.7", "v0.2.0"] }), "0.2.1");
    assert.equal(buildVersion({ ref: main, base: "0.3.0", tags: ["v0.1.7", "v0.2.0"] }), "0.3.1");
    assert.equal(
      buildVersion({ ref: main, base: "0.1.0", tags: ["v1.0.0-rc.1", "v0.1.7"] }),
      "0.1.8",
    );
  });

  it("is the tag's for a v* tag pushed by hand", () => {
    assert.equal(
      buildVersion({ ref: "refs/tags/v0.2.0", base: "0.1.0", tags: ["v0.1.7", "v0.2.0"] }),
      "0.2.0",
    );
    assert.equal(buildVersion({ ref: "refs/tags/v1.0.0-rc.1", base: "0.1.0" }), "1.0.0-rc.1");
    assert.throws(() => buildVersion({ ref: "refs/tags/vnext", base: "0.1.0" }), /not a version/);
    assert.throws(() => buildVersion({ ref: "refs/tags/v1.2", base: "0.1.0" }), /not a version/);
    assert.throws(() => buildVersion({ ref: main, base: "0.1" }), /MAJOR\.MINOR\.PATCH/);
    assert.throws(() => buildVersion({ ref: main, base: "0.1.0-beta" }), /MAJOR\.MINOR\.PATCH/);
  });

  it("names the channel", () => {
    assert.equal(channelOf(main, "0.1.7"), "nightly");
    assert.equal(channelOf("", "0.1.7"), "nightly");
    assert.equal(channelOf("refs/tags/v0.2.0", "0.2.0"), "stable");
    assert.equal(channelOf("refs/tags/v1.0.0-rc.1", "1.0.0-rc.1"), null);
    assert.equal(
      endpointOf("stable", "kungfufafa/tabler"),
      "https://github.com/kungfufafa/tabler/releases/latest/download/latest.json",
    );
    assert.equal(
      endpointOf("nightly", "kungfufafa/tabler"),
      "https://github.com/kungfufafa/tabler/releases/download/nightly/latest.json",
    );
    assert.equal(endpointOf(null, "kungfufafa/tabler"), null);
  });

  it("gives a pre-release tag an MSI version of numbers only", () => {
    assert.equal(wixVersion("0.1.7"), null);
    assert.equal(wixVersion("1.0.0-rc.1"), "1.0.0");
  });
});

describe("the version files", () => {
  const read = (file) => fs.readFileSync(path.join(ROOT, file), "utf8");

  it("state one version, a plain one", () => {
    const versions = VERSION_FILES.map((file) => versionIn(file, read(file)));
    assert.equal(new Set(versions).size, 1, `${VERSION_FILES.join(", ")}: ${versions.join(", ")}`);
    assert.ok(isPlain(versions[0]), versions[0]);
  });

  it("take the version of a build, and nothing else changes", () => {
    for (const file of VERSION_FILES) {
      const before = read(file);
      const after = withVersion(file, before, "0.1.42");
      assert.equal(versionIn(file, after), "0.1.42", file);
      const changed = after.split("\n").filter((line, i) => line !== before.split("\n")[i]);
      assert.equal(changed.length, 1, `${file}: ${changed.join(" | ")}`);
      assert.match(changed[0], /version/);
    }
    // Only the package's own version in Cargo.toml, not a dependency's.
    const cargo = withVersion("src-tauri/Cargo.toml", read("src-tauri/Cargo.toml"), "9.9.9");
    assert.equal(cargo.match(/9\.9\.9/g).length, 1);
    assert.match(cargo, /^\[package\]\nname = "tabler"\nversion = "9\.9\.9"/m);
    assert.throws(() => withVersion("package.json", "{}", "1.0.0"), /no "version"/);
    assert.throws(
      () => withVersion("Cargo.toml", '[dependencies]\nversion = "1"', "1.0.0"),
      /\[package\]/,
    );
  });

  it("keep tauri.conf.json a base the builds can start from", () => {
    const conf = JSON.parse(read("src-tauri/tauri.conf.json"));
    // A build made by hand has no signing key: it must not try to sign updater artifacts.
    assert.equal(conf.bundle.createUpdaterArtifacts, false);
    assert.equal(conf.plugins.updater.requireSignedVersion, true);
    for (const endpoint of conf.plugins.updater.endpoints) assert.match(endpoint, /^https:\/\//);
  });
});

describe("the config of a build", () => {
  const repo = "kungfufafa/tabler";

  it("turns the updater on for a signed build of main, on the nightly channel", () => {
    assert.deepEqual(
      releaseConfig({ ref: "refs/heads/main", version: "0.1.7", repo, signed: true }),
      {
        version: "0.1.7",
        bundle: { createUpdaterArtifacts: true },
        plugins: {
          updater: {
            endpoints: [
              "https://github.com/kungfufafa/tabler/releases/download/nightly/latest.json",
            ],
          },
        },
      },
    );
  });

  it("puts a signed release tagged by hand on the stable channel", () => {
    assert.deepEqual(
      releaseConfig({ ref: "refs/tags/v0.2.0", version: "0.2.0", repo, signed: true }),
      {
        version: "0.2.0",
        bundle: { createUpdaterArtifacts: true },
        plugins: {
          updater: {
            endpoints: [
              "https://github.com/kungfufafa/tabler/releases/latest/download/latest.json",
            ],
          },
        },
      },
    );
  });

  it("follows the releases of the repository that built it", () => {
    const config = releaseConfig({
      ref: "refs/heads/main",
      version: "0.1.7",
      repo: "someone/fork",
      signed: true,
    });
    assert.deepEqual(config.plugins.updater.endpoints, [
      "https://github.com/someone/fork/releases/download/nightly/latest.json",
    ]);
  });

  it("gives a build without the signing key no updater artifacts and no update address", () => {
    for (const [ref, version] of [
      ["refs/heads/main", "0.1.7"],
      ["refs/tags/v0.2.0", "0.2.0"],
    ]) {
      const config = releaseConfig({ ref, version, repo, signed: false });
      assert.equal(config.bundle.createUpdaterArtifacts, false);
      assert.deepEqual(config.plugins.updater.endpoints, []);
    }
  });

  it("signs a pre-release tag but puts it on no channel", () => {
    const config = releaseConfig({
      ref: "refs/tags/v1.0.0-rc.1",
      version: "1.0.0-rc.1",
      repo,
      signed: true,
    });
    assert.deepEqual(config.plugins.updater.endpoints, []);
    assert.deepEqual(config.bundle.windows, { wix: { version: "1.0.0" } });
    assert.throws(
      () => releaseConfig({ ref: "", version: "soon", repo, signed: true }),
      /not a version/,
    );
  });

  it("prints the version from the tags it is given", () => {
    const script = path.join(HERE, "release-config.mjs");
    const run = (...args) =>
      spawnSync(process.execPath, [script, "--version", ...args], {
        encoding: "utf8",
        env: { ...process.env, GITHUB_OUTPUT: "", GITHUB_REF: "" },
      });
    const base = versionIn(
      "src-tauri/tauri.conf.json",
      fs.readFileSync(path.join(ROOT, "src-tauri/tauri.conf.json"), "utf8"),
    );
    const { major, minor, patch } = parseVersion(base);
    const none = run("--ref", "refs/heads/main", "--tags", "nightly", "--head-tags", "");
    assert.equal(none.status, 0, none.stderr);
    assert.equal(none.stdout.trim(), `${major}.${minor}.${patch + 1}`);
    const next = run(
      "--ref",
      "refs/heads/main",
      "--tags",
      "v7.0.1,v7.0.2,nightly",
      "--head-tags",
      "nightly",
    );
    assert.equal(next.stdout.trim(), "7.0.3");
    const tag = run(
      "--ref",
      "refs/tags/v7.1.0",
      "--tags",
      "v7.0.1,v7.1.0",
      "--head-tags",
      "v7.1.0",
    );
    assert.equal(tag.stdout.trim(), "7.1.0");
  });
});

describe("a CI checkout", () => {
  it("gets the build's version in its three files and the updater config beside them", () => {
    // A copy of what the script reads and writes, so the repository's own files stay as they are.
    const checkout = fs.mkdtempSync(path.join(TMP, "checkout-"));
    fs.mkdirSync(path.join(checkout, "dev/scripts"), { recursive: true });
    fs.mkdirSync(path.join(checkout, "src-tauri"));
    for (const script of ["release-config.mjs", "build-version.mjs"])
      fs.copyFileSync(path.join(HERE, script), path.join(checkout, "dev/scripts", script));
    for (const file of [...VERSION_FILES, "src-tauri/Cargo.lock"])
      fs.copyFileSync(path.join(ROOT, file), path.join(checkout, file));
    const out = path.join(checkout, "release.conf.json");
    const run = spawnSync(
      process.execPath,
      [
        path.join(checkout, "dev/scripts/release-config.mjs"),
        "--apply",
        "--build-version",
        "0.1.42",
        "--out",
        out,
        "--ref",
        "refs/heads/main",
        "--repo",
        "kungfufafa/tabler",
        "--signed",
        "true",
      ],
      { encoding: "utf8", env: { ...process.env, GITHUB_OUTPUT: "" } },
    );
    assert.equal(run.status, 0, run.stderr);
    for (const file of VERSION_FILES)
      assert.equal(
        versionIn(file, fs.readFileSync(path.join(checkout, file), "utf8")),
        "0.1.42",
        file,
      );
    // Still valid files: JSON parses, and Cargo.toml only changed its version line.
    assert.equal(
      JSON.parse(fs.readFileSync(path.join(checkout, "package.json"), "utf8")).version,
      "0.1.42",
    );
    assert.equal(
      JSON.parse(fs.readFileSync(path.join(checkout, "src-tauri/tauri.conf.json"), "utf8")).version,
      "0.1.42",
    );
    assert.deepEqual(JSON.parse(fs.readFileSync(out, "utf8")), {
      version: "0.1.42",
      bundle: { createUpdaterArtifacts: true },
      plugins: {
        updater: {
          endpoints: ["https://github.com/kungfufafa/tabler/releases/download/nightly/latest.json"],
        },
      },
    });
    // The repository's own files were not touched.
    for (const file of VERSION_FILES)
      assert.notEqual(
        versionIn(file, fs.readFileSync(path.join(ROOT, file), "utf8")),
        "0.1.42",
        file,
      );
  });
});

describe("the notes of a version", () => {
  it("sorts commits into what a user notices", () => {
    assert.deepEqual(classify("feat: updates install from inside the app"), {
      group: "New",
      text: "Updates install from inside the app",
    });
    assert.deepEqual(classify("fix(grid): the sidebar keeps its width"), {
      group: "Fixed",
      text: "The sidebar keeps its width",
    });
    assert.deepEqual(classify("perf: faster first paint"), {
      group: "Improved",
      text: "Faster first paint",
    });
    assert.deepEqual(classify("feat!: a new file format"), {
      group: "New",
      text: "A new file format",
    });
    assert.deepEqual(classify("ui: the grid gutter as TablePlus's"), {
      group: "Improved",
      text: "The grid gutter as TablePlus's",
    });
    // A type the notes do not know stays in the text: it says what the change is about.
    assert.deepEqual(classify("redis: keys list by pattern"), {
      group: "Changed",
      text: "Redis: keys list by pattern",
    });
    assert.deepEqual(classify("the Welcome window opens centred"), {
      group: "Changed",
      text: "The Welcome window opens centred",
    });
    for (const hidden of [
      "ci: the Windows checks run as two jobs",
      "docs: the update flow",
      "test: more grid cases",
      "chore(deps): bump vite",
      "Merge branch 'main'",
      "",
    ])
      assert.equal(classify(hidden), null, hidden);
  });

  it("groups a version's changes, each once, and caps a long group", () => {
    const groups = groupChanges(["fix: B", "feat: A", "ci: nothing to see", "fix: C", "fix: B"]);
    assert.deepEqual(groups, [
      { group: "New", items: ["A"], more: 0 },
      { group: "Fixed", items: ["B", "C"], more: 0 },
    ]);
    const many = groupChanges(Array.from({ length: MAX_ITEMS + 5 }, (_, i) => `fix: bug ${i}`));
    assert.equal(many[0].items.length, MAX_ITEMS);
    assert.equal(many[0].more, 5);
  });

  it("writes the notes for the update dialog, newest version first", () => {
    const text = renderNotes([
      {
        version: "0.1.3",
        subjects: [
          "feat: updates install from inside the app",
          "fix: the sidebar keeps its width",
          "ci: faster builds",
        ],
      },
      { version: "0.1.2", subjects: ["ci: only housekeeping"] },
      { version: "0.1.1", subjects: ["fix: quitting keeps unsaved work"] },
    ]);
    assert.equal(
      text,
      [
        "0.1.3",
        "",
        "New",
        "- Updates install from inside the app",
        "",
        "Fixed",
        "- The sidebar keeps its width",
        "",
        "0.1.2",
        "",
        NO_CHANGES,
        "",
        "0.1.1",
        "",
        "Fixed",
        "- Quitting keeps unsaved work",
        "",
      ].join("\n"),
    );
  });

  it("writes the same notes as Markdown for the GitHub release", () => {
    const md = renderNotes([{ version: "0.1.3", subjects: ["feat: A", "fix: B"] }], "markdown");
    assert.equal(md, "## 0.1.3\n\n**New**\n\n- A\n\n**Fixed**\n\n- B\n");
  });

  it("keeps a squash-merged pull request's number on the release page only", () => {
    assert.deepEqual(classify("fix(grid): the sidebar keeps its width (#14)"), {
      group: "Fixed",
      text: "The sidebar keeps its width",
    });
    // Only the number GitHub appends is taken off; a reference inside the subject is part of it.
    assert.deepEqual(classify("fix: the crash of #9 on quit"), {
      group: "Fixed",
      text: "The crash of #9 on quit",
    });
    assert.equal(classify("ci: check pull request titles (#15)"), null);
    // The same change with and without a number is one change, as its newest commit has it.
    const subjects = ["feat: A (#16)", "fix: B", "fix: B (#12)"];
    assert.equal(
      renderNotes([{ version: "0.1.3", subjects }]),
      "0.1.3\n\nNew\n- A\n\nFixed\n- B\n",
    );
    assert.equal(
      renderNotes([{ version: "0.1.3", subjects }], "markdown"),
      "## 0.1.3\n\n**New**\n\n- A (#16)\n\n**Fixed**\n\n- B\n",
    );
  });

  it("takes each version's commits from the tag before it", () => {
    const tags = ["nightly", "v0.1.1", "v0.1.2", "v0.1.3"];
    assert.deepEqual(versionRanges({ version: "0.1.4", head: "HEAD", tags, count: 3 }), [
      { version: "0.1.4", to: "HEAD", from: "v0.1.3" },
      { version: "0.1.3", to: "v0.1.3", from: "v0.1.2" },
      { version: "0.1.2", to: "v0.1.2", from: "v0.1.1" },
    ]);
    // The first version ever has no tag before it; a version built again does not list itself.
    assert.deepEqual(
      versionRanges({ version: "0.1.1", head: "HEAD", tags: ["nightly"], count: 8 }),
      [{ version: "0.1.1", to: "HEAD", from: null }],
    );
    assert.deepEqual(versionRanges({ version: "0.1.3", head: "HEAD", tags, count: 8 }), [
      { version: "0.1.3", to: "HEAD", from: "v0.1.2" },
      { version: "0.1.2", to: "v0.1.2", from: "v0.1.1" },
      { version: "0.1.1", to: "v0.1.1", from: null },
    ]);
  });

  it("writes this repository's notes from its history", () => {
    const run = spawnSync(
      process.execPath,
      [path.join(HERE, "release-notes.mjs"), "--version", "0.1.1", "--versions", "1"],
      { encoding: "utf8" },
    );
    assert.equal(run.status, 0, run.stderr);
    assert.match(run.stdout, /^0\.1\.1\n\n/);
  });
});

describe("latest.json", () => {
  const baseUrl = "https://github.com/kungfufafa/tabler/releases/download/nightly";
  const version = "0.1.7";
  const files = [
    { name: "Tabler.app.tar.gz", signature: sig("Tabler.app.tar.gz", version) },
    { name: "Tabler_0.1.7_x64-setup.exe", signature: sig("setup.exe", version) },
    { name: "Tabler_0.1.7_x64_en-US.msi", signature: sig("x.msi", version) },
  ];

  it("maps each updater bundle to the targets the app asks for", () => {
    assert.deepEqual(targetsOf("Tabler.app.tar.gz"), [
      "darwin-aarch64-app",
      "darwin-x86_64-app",
      "darwin-aarch64",
      "darwin-x86_64",
    ]);
    assert.deepEqual(targetsOf("Tabler_0.1.0_x64-setup.exe"), [
      "windows-x86_64-nsis",
      "windows-x86_64",
    ]);
    assert.deepEqual(targetsOf("Tabler_0.1.0_x64_en-US.msi"), ["windows-x86_64-msi"]);
    assert.deepEqual(targetsOf("Tabler_0.1.0_universal.dmg"), []);
    assert.deepEqual(targetsOf("latest.json"), []);
  });

  it("reads the version a signature was made for", () => {
    assert.equal(signedVersion(sig("a", "0.1.7")), "0.1.7");
    assert.equal(signedVersion(sig("a", "")), null);
    assert.equal(signedVersion("bm90IGEgc2lnbmF0dXJl"), null);
  });

  it("lists every platform with its file and signature", () => {
    const manifest = buildManifest({
      version,
      notes: "- feat: updates\n",
      pubDate: "2026-11-12T08:00:00.000Z",
      baseUrl: `${baseUrl}/`,
      files,
    });
    assert.equal(manifest.version, version);
    assert.equal(manifest.notes, "- feat: updates");
    assert.equal(manifest.pub_date, "2026-11-12T08:00:00.000Z");
    assert.deepEqual(Object.keys(manifest.platforms).sort(), [
      "darwin-aarch64",
      "darwin-aarch64-app",
      "darwin-x86_64",
      "darwin-x86_64-app",
      "windows-x86_64",
      "windows-x86_64-msi",
      "windows-x86_64-nsis",
    ]);
    assert.equal(manifest.platforms["darwin-aarch64"].url, `${baseUrl}/Tabler.app.tar.gz`);
    assert.equal(manifest.platforms["darwin-x86_64"].signature, files[0].signature);
    assert.equal(
      manifest.platforms["windows-x86_64-nsis"].url,
      `${baseUrl}/Tabler_0.1.7_x64-setup.exe`,
    );
    assert.equal(manifest.platforms["windows-x86_64"], manifest.platforms["windows-x86_64-nsis"]);
    assert.equal(
      manifest.platforms["windows-x86_64-msi"].url,
      `${baseUrl}/Tabler_0.1.7_x64_en-US.msi`,
    );
  });

  it("is not written for a release without signed bundles", () => {
    assert.equal(buildManifest({ version, notes: "", pubDate: "", baseUrl, files: [] }), null);
  });

  it("refuses a bundle signed for another version, or for none", () => {
    const other = [{ name: "Tabler.app.tar.gz", signature: sig("Tabler.app.tar.gz", "0.1.6") }];
    assert.throws(
      () => buildManifest({ version, notes: "", pubDate: "", baseUrl, files: other }),
      /signed for 0\.1\.6, but the release is 0\.1\.7/,
    );
    const none = [{ name: "Tabler.app.tar.gz", signature: sig("Tabler.app.tar.gz", "") }];
    assert.throws(
      () => buildManifest({ version, notes: "", pubDate: "", baseUrl, files: none }),
      /names no version/,
    );
  });

  it("builds from a folder of release files, and writes nothing without signatures", () => {
    const script = path.join(HERE, "updater-manifest.mjs");
    const dir = fs.mkdtempSync(path.join(TMP, "files-"));
    fs.writeFileSync(path.join(dir, "Tabler_0.1.7_universal.dmg"), "dmg");
    const args = ["--dir", dir, "--version", version, "--base-url", baseUrl];
    const unsigned = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
    assert.equal(unsigned.status, 0, unsigned.stderr);
    assert.ok(!fs.existsSync(path.join(dir, "latest.json")));

    for (const { name, signature } of files) {
      fs.writeFileSync(path.join(dir, name), "bundle");
      fs.writeFileSync(path.join(dir, `${name}.sig`), signature);
    }
    fs.writeFileSync(path.join(dir, "notes.txt"), "- feat: updates\n- fix: sidebar\n");
    const signed = spawnSync(
      process.execPath,
      [script, ...args, "--notes-file", path.join(dir, "notes.txt")],
      { encoding: "utf8" },
    );
    assert.equal(signed.status, 0, signed.stderr);
    const manifest = JSON.parse(fs.readFileSync(path.join(dir, "latest.json"), "utf8"));
    assert.equal(manifest.version, version);
    assert.equal(manifest.notes, "- feat: updates\n- fix: sidebar");
    assert.ok(!Number.isNaN(Date.parse(manifest.pub_date)));
    assert.equal(Object.keys(manifest.platforms).length, 7);
  });
});
