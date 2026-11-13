import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { test } from "node:test";

import { EXAMPLE, TYPES, parseTitle, refusal } from "./pr-title.mjs";

const script = path.join(import.meta.dirname, "pr-title.mjs");

/** The check as the workflow runs it: the title in PR_TITLE, nothing on the command line. */
const check = (title, env = {}) =>
  spawnSync(process.execPath, [script], {
    encoding: "utf8",
    env: { ...process.env, GITHUB_ACTIONS: "", ...env, PR_TITLE: title },
  });

test("accepts every type, with a scope and with a breaking mark", () => {
  for (const type of TYPES) {
    assert.deepEqual(parseTitle(`${type}: keep the query editor focused`), {
      type,
      scope: null,
      breaking: false,
      subject: "keep the query editor focused",
    });
  }
  assert.ok(parseTitle(EXAMPLE));
  assert.deepEqual(parseTitle("fix(windows): native window chrome on Windows"), {
    type: "fix",
    scope: "windows",
    breaking: false,
    subject: "native window chrome on Windows",
  });
  assert.equal(parseTitle("feat(query editor): run the selection").scope, "query editor");
  assert.equal(parseTitle("feat!: drop the legacy connection file").breaking, true);
  assert.equal(parseTitle("feat(ipc)!: values cross as tagged enums").breaking, true);
  // What follows the type is the author's: a colon, parentheses or a number are part of it.
  assert.equal(parseTitle("ci: check pull request titles (#14)").subject.endsWith("(#14)"), true);
  assert.equal(parseTitle("docs: README: the release steps").subject, "README: the release steps");
});

test("refuses a title the release notes could not sort", () => {
  const refused = [
    "",
    "   ",
    "Keep the query editor focused",
    "M8 wave 3 + quit crash fix + Gajah Sel logo (bring main in line)",
    'Revert "fix: keep the query editor focused"',
    "Merge branch 'main' into windows-chrome",
    // A type the release notes do not know, or know under another spelling.
    "redis: list the keys of a database",
    "feature: sort by click",
    "tests: cover the grid",
    "style: reformat",
    "Fix: keep the query editor focused",
    "FEAT: sort by click",
    // The type is there, the rest of the form is not.
    "fix keep the query editor focused",
    "fix:keep the query editor focused",
    "fix:",
    "fix: ",
    "fix:  keep the query editor focused",
    "fix (windows): native window chrome",
    "fix(): native window chrome",
    "fix( ): native window chrome",
    "fix(win(dows)): native window chrome",
    "fix(windows) : native window chrome",
    "fix!!: native window chrome",
    " fix: keep the query editor focused",
    "fix: keep the query editor focused\nfeat: and a second line",
  ];
  for (const title of refused) assert.equal(parseTitle(title), null, JSON.stringify(title));
  assert.equal(parseTitle(undefined), null);
  assert.equal(parseTitle(null), null);
});

test("the refusal names the title, the types and an example", () => {
  const lines = refusal("Keep the query editor focused");
  assert.match(lines[0], /"Keep the query editor focused"/);
  assert.ok(lines.includes(`Types: ${TYPES.join(", ")}`));
  assert.ok(lines.includes(`Example: ${EXAMPLE}`));
});

test("exits 0 for a good title and 1 for any other, as the workflow step sees it", () => {
  const good = check(EXAMPLE);
  assert.equal(good.status, 0, good.stderr);
  assert.match(good.stdout, /fix: keep the query editor focused/);

  const bad = check("Keep the query editor focused");
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /"Keep the query editor focused"/);
  for (const type of TYPES) assert.match(bad.stderr, new RegExp(`\\b${type}\\b`));
  assert.match(bad.stderr, /Example: fix: keep the query editor focused/);
  // Outside Actions nothing is printed for the runner.
  assert.equal(bad.stdout, "");

  assert.equal(check("").status, 1);
  const env = { ...process.env };
  delete env.PR_TITLE;
  const unset = spawnSync(process.execPath, [script], { encoding: "utf8", env });
  assert.equal(unset.status, 1);
  assert.match(unset.stderr, /cannot be merged: ""/);
});

test("a title cannot give the Actions runner a command", () => {
  // The runner reads a line that starts with `::` (after spaces) as a command.
  const titles = ["::add-mask::fix: x", "  ::stop-commands::now", "::error::fix: x\n::warning::y"];
  for (const title of titles) {
    const run = check(title, { GITHUB_ACTIONS: "true" });
    assert.equal(run.status, 1);
    const commands = `${run.stdout}\n${run.stderr}`
      .split(/\r?\n/)
      .filter((line) => line.trimStart().startsWith("::"));
    assert.equal(commands.length, 1, commands.join("\n"));
    assert.match(commands[0], /^::error title=Pull request title::Write the title as/);
    assert.equal(commands[0].includes("add-mask"), false);
    assert.equal(commands[0].includes("stop-commands"), false);
  }
});
