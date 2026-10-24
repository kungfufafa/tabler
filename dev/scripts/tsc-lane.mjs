#!/usr/bin/env node
// dev/scripts/tsc-lane.mjs -- `tsc --noEmit` that fails only on what the caller owns
// (docs/drivers/README.md).
//
//   node dev/scripts/tsc-lane.mjs <lane>          diagnostics in shared files and in the lane's own
//   node dev/scripts/tsc-lane.mjs --foundation    diagnostics in shared files only
//
// The other lanes' unfinished files share the checkout: their diagnostics are counted, not
// failed. A lane's files are its renderer folders, its bindings (`src/lib/ipc/bindings/<Prefix>*`)
// and its docs, as dev/lanes.json lists them. Holds one of the two TS slots while tsc runs.

import { TS_SLOTS, UsageError, acquireAny, getLane, readLanes, tscCheck } from "./lane-lib.mjs";

async function main() {
  const [arg, ...extra] = process.argv.slice(2);
  if (!arg || extra.length) throw new UsageError("usage: tsc-lane.mjs <lane> | --foundation");
  const lanes = readLanes();
  const lane = arg === "--foundation" ? null : getLane(lanes, arg).name;
  const who = lane ?? "foundation";
  await acquireAny(TS_SLOTS, `tsc ${who}`);
  const { failing, ignored } = await tscCheck(lanes, lane);
  for (const d of failing) console.log(d.text);
  const others = Object.entries(ignored).map(([l, n]) => `${l} ${n}`);
  if (others.length) console.log(`[tsc-lane] ignored lane diagnostics: ${others.join(", ")}`);
  const where = lane ? `shared or ${lane} files` : "shared files";
  console.log(`[tsc-lane] ${who}: ${failing.length} diagnostic(s) in ${where}`);
  return failing.length ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`[tsc-lane] ${e instanceof UsageError ? "" : "error: "}${e.message}`);
    process.exit(e instanceof UsageError ? 2 : 1);
  },
);
