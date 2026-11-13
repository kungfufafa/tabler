// dev/scripts/pr-title.mjs -- whether a pull request title is one `main` can take
// (.github/workflows/pr-title.yml):
//
//   PR_TITLE="fix: keep the query editor focused" node dev/scripts/pr-title.mjs
//
// A pull request is squash-merged, so its title becomes the commit subject on `main`, and the
// release notes sort those subjects by their type: `feat` is "New", `fix` is "Fixed", `ci` is left
// out. A title without a type would be listed as it stands, under "Changed".
import fs from "node:fs";
import { fileURLToPath } from "node:url";

/** The types a title starts with: the ones the release notes know, in one spelling each. */
export const TYPES = [
  "feat",
  "fix",
  "ui",
  "perf",
  "refactor",
  "docs",
  "test",
  "ci",
  "build",
  "chore",
];

/** A title the check accepts. */
export const EXAMPLE = "fix: keep the query editor focused";

// `type`, `type(scope)` or either with `!`, then `: ` and a subject. The scope is whatever
// stands between the parentheses (`windows`, `query editor`), as long as it is something.
const TITLE = new RegExp(`^(${TYPES.join("|")})(?:\\(([^()]*[^()\\s][^()]*)\\))?(!)?: (\\S.*)$`);

/**
 * The parts of `type(scope)!: subject` as `{ type, scope, breaking, subject }` (`scope` is null
 * without one), or null for a title that is not written that way.
 */
export function parseTitle(title) {
  const m = TITLE.exec(title ?? "");
  if (!m) return null;
  return { type: m[1], scope: m[2] ?? null, breaking: m[3] === "!", subject: m[4] };
}

/** What the check prints for a title it refuses, one line per entry. */
export function refusal(title) {
  // The title is quoted and never starts a line: a line that starts with `::` is a command to
  // the Actions runner, and a title can be made to start with one.
  return [
    `This pull request title cannot be merged: ${JSON.stringify(title ?? "")}`,
    "It becomes the commit subject on main, and the release notes sort commits by their type.",
    "Write it as `type: subject` or `type(scope): subject` (`!` before the colon marks a breaking change).",
    `Types: ${TYPES.join(", ")}`,
    `Example: ${EXAMPLE}`,
  ];
}

function main() {
  const title = process.env.PR_TITLE ?? "";
  if (parseTitle(title)) {
    console.log(`The pull request title is fine: ${JSON.stringify(title)}`);
    return;
  }
  for (const line of refusal(title)) console.error(line);
  // The same in the run's summary, without the title: an annotation is a runner command too.
  if (process.env.GITHUB_ACTIONS)
    console.log(
      `::error title=Pull request title::Write the title as "type: subject", with one of: ${TYPES.join(", ")}. Example: ${EXAMPLE}`,
    );
  process.exitCode = 1;
}

if (
  process.argv[1] &&
  fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))
)
  main();
