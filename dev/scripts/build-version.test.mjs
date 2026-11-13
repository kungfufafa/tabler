import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { VERSION_FILES, buildVersion, versionIn } from "./build-version.mjs";

const root = path.resolve(import.meta.dirname, "../..");
const main = { ref: "refs/heads/main", base: "0.1.0" };

test("increments the highest patch number and reuses a commit's version", () => {
  assert.equal(buildVersion({ ...main, tags: ["nightly"] }), "0.1.1");
  assert.equal(buildVersion({ ...main, tags: ["v0.1.9", "v0.1.10", "v0.2.0-rc.1"] }), "0.1.11");
  assert.equal(buildVersion({ ...main, tags: ["v0.2.0"] }), "0.2.1");
  assert.equal(buildVersion({ ...main, tags: ["v0.2.0"], headTags: ["v0.1.2"] }), "0.1.2");
  assert.equal(buildVersion({ ...main, ref: "refs/tags/v0.2.0" }), "0.2.0");
  assert.equal(buildVersion({ ...main, ref: "refs/tags/v0.2.0-rc.1" }), "0.2.0-rc.1");
});

test("versions both platform builds and reserves the patch on the published commit", (t) => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "tabler-build-version-"));
  t.after(() => fs.rmSync(temp, { recursive: true, force: true }));
  const checkout = path.join(temp, "checkout");
  const remote = path.join(temp, "remote.git");
  const output = path.join(temp, "output");
  const config = path.join(temp, "release.json");
  const script = "dev/scripts/build-version.mjs";
  const lock = "src-tauri/Cargo.lock";
  for (const file of [...VERSION_FILES, script, lock]) {
    const target = path.join(checkout, file);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(path.join(root, file), target);
  }

  const git = (...args) => execFileSync("git", args, { cwd: checkout, encoding: "utf8" }).trim();
  const run = (...args) => {
    fs.writeFileSync(output, "");
    return spawnSync(process.execPath, [script, ...args], {
      cwd: checkout,
      encoding: "utf8",
      env: { ...process.env, GITHUB_REF: main.ref, GITHUB_OUTPUT: output },
    });
  };
  const expectSuccess = (...args) => {
    const result = run(...args);
    assert.equal(result.status, 0, result.stderr);
    return fs.readFileSync(output, "utf8").trim();
  };

  git("init", "--bare", remote);
  git("init", "-b", "main");
  git("config", "user.email", "release-test@example.invalid");
  git("config", "user.name", "Release Test");
  git("config", "commit.gpgsign", "false");
  git("config", "tag.gpgSign", "false");
  git("add", ".");
  git("commit", "-m", "First build");
  git("remote", "add", "origin", remote);
  git("push", "origin", "main");
  const first = git("rev-parse", "HEAD");
  const firstVersion = buildVersion({
    ...main,
    base: versionIn(
      "src-tauri/tauri.conf.json",
      fs.readFileSync(path.join(checkout, "src-tauri/tauri.conf.json"), "utf8"),
    ),
  });
  assert.equal(expectSuccess("--version"), `version=${firstVersion}`);

  const originalLock = fs.readFileSync(path.join(checkout, lock), "utf8");
  expectSuccess("--apply", firstVersion, config);
  assert.deepEqual(JSON.parse(fs.readFileSync(config, "utf8")), { version: firstVersion });
  for (const file of VERSION_FILES)
    assert.equal(versionIn(file, fs.readFileSync(path.join(checkout, file), "utf8")), firstVersion);
  assert.equal(
    fs.readFileSync(path.join(checkout, lock), "utf8"),
    originalLock.replace(
      /(\[\[package\]\]\r?\nname = "tabler"\r?\nversion = )"[^"]*"/,
      `$1"${firstVersion}"`,
    ),
  );
  git("restore", ...VERSION_FILES, lock);

  assert.equal(expectSuccess("--tag-nightly", firstVersion), "publish=true");
  assert.equal(git("ls-remote", "--tags", "origin", `v${firstVersion}`).split(/\s/)[0], first);
  git("push", "origin", "HEAD:refs/tags/nightly");
  assert.equal(expectSuccess("--tag-nightly", firstVersion), "publish=true");

  git("commit", "--allow-empty", "-m", "Second build");
  git("push", "origin", "main");
  const secondVersion = buildVersion({ ...main, tags: [`v${firstVersion}`] });
  assert.equal(expectSuccess("--version"), `version=${secondVersion}`);
  assert.equal(expectSuccess("--tag-nightly", secondVersion), "publish=true");
  git("push", "--force", "origin", "HEAD:refs/tags/nightly");

  git("checkout", "--detach", first);
  assert.equal(expectSuccess("--tag-nightly", firstVersion), "publish=false");

  git("checkout", "main");
  git("commit", "--allow-empty", "-m", "Third build");
  const thirdVersion = buildVersion({ ...main, tags: [`v${secondVersion}`] });
  assert.equal(expectSuccess("--version"), `version=${thirdVersion}`);
  git("push", "origin", `${first}:refs/tags/v${thirdVersion}`);
  const collision = run("--tag-nightly", thirdVersion);
  assert.equal(collision.status, 1);
  assert.match(collision.stderr, /Rerun all jobs to rebuild/);
  assert.equal(git("ls-remote", "--tags", "origin", `v${thirdVersion}`).split(/\s/)[0], first);
});
