import { describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { groupProfiles, isGroupCollapsed, visibleProfiles } from "./list";

const names = (groups: ReturnType<typeof groupProfiles>) =>
  groups.map(([g, items]) => [g, items.map((p) => p.name)]);

describe("groupProfiles", () => {
  it("puts the ungrouped connections first, then the groups alphabetically, keeping the order inside", () => {
    expect(names(groupProfiles(SEED_PROFILES, ""))).toEqual([
      ["", ["sample.sqlite"]],
      ["Local", ["Local Postgres", "Shop MySQL"]],
      ["Production", ["Production (read-only)"]],
      ["Staging", ["Staging via SSH", "Rotated host"]],
    ]);
  });

  it("searches names, the host : port : user : database line, groups and tags", () => {
    expect(names(groupProfiles(SEED_PROFILES, "  SHOP "))).toEqual([["Local", ["Shop MySQL"]]]);
    expect(names(groupProfiles(SEED_PROFILES, "10.0.2.15"))).toEqual([
      ["Staging", ["Rotated host"]],
    ]);
    expect(names(groupProfiles(SEED_PROFILES, "development"))).toEqual([["Local", ["Shop MySQL"]]]);
    expect(names(groupProfiles(SEED_PROFILES, "fixtures/sample"))).toEqual([
      ["", ["sample.sqlite"]],
    ]);
    expect(groupProfiles(SEED_PROFILES, "nothing like this")).toEqual([]);
  });
});

describe("visibleProfiles", () => {
  const ids = (rows: ReturnType<typeof visibleProfiles>) => rows.map((p) => p.id);
  const all = groupProfiles(SEED_PROFILES, "");

  it("leaves out the rows of collapsed groups, keeping the list order", () => {
    expect(ids(visibleProfiles(all, ["Local", "Staging"], false))).toEqual([
      "p-sample-sqlite",
      "p-prod-pg",
    ]);
    expect(ids(visibleProfiles(all, [], false))).toHaveLength(SEED_PROFILES.length);
  });

  it("never hides the ungrouped connections (they have no header to expand them again)", () => {
    expect(isGroupCollapsed("", [""], false)).toBe(false);
    expect(ids(visibleProfiles(all, ["", "Local", "Production", "Staging"], false))).toEqual([
      "p-sample-sqlite",
    ]);
  });

  it("shows every match while searching", () => {
    expect(isGroupCollapsed("Staging", ["Staging"], true)).toBe(false);
    expect(
      ids(visibleProfiles(groupProfiles(SEED_PROFILES, "staging"), ["Staging"], true)),
    ).toEqual(["p-staging-ssh", "p-rotated-ssh"]);
  });
});
