import { describe, expect, it, vi } from "vitest";
import { parseConnectionUrl } from "@/features/connections/url";
import lane from ".";
import type { FlavorKind } from "./url";

// Where a flavor's URL comes from goes on to the PostgreSQL reader: a clicked link reads as a
// link there too, and a reader asked without a source reads it as one.

vi.mock("@/features/connections/url", async (original) => {
  const real = await original<typeof import("@/features/connections/url")>();
  return { ...real, parseConnectionUrl: vi.fn(real.parseConnectionUrl) };
});

const KINDS: FlavorKind[] = ["cockroachdb", "greenplum", "redshift"];

describe("the flavors' URL source", () => {
  it("goes on to the PostgreSQL reader", () => {
    for (const kind of KINDS)
      for (const [source, sent] of [
        ["paste", "paste"],
        ["link", "link"],
        [undefined, "link"],
      ] as const) {
        vi.mocked(parseConnectionUrl).mockClear();
        const parsed = lane.kinds![kind]!.url!.parse(`${kind}://u@h:1/db`, source);
        expect(parsed?.values.pgFlavor).toBeTruthy();
        expect(vi.mocked(parseConnectionUrl).mock.calls).toEqual([["postgresql://u@h:1/db", sent]]);
      }
  });
});
