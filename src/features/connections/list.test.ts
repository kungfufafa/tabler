import { describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import {
  groupDropId,
  groupOfDropTarget,
  groupProfiles,
  isGroupCollapsed,
  sortProfiles,
  visibleProfiles,
} from "./list";

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

describe("sorting and empty groups", () => {
  const order = (sort: Parameters<typeof sortProfiles>[1]) =>
    sortProfiles(SEED_PROFILES, sort).map((p) => p.name);

  it("sorts by name, tag (TablePlus's order, untagged last), driver or the saved order", () => {
    expect(order("manual")).toEqual(SEED_PROFILES.map((p) => p.name));
    expect(order("name")).toEqual([
      "Local Postgres",
      "Production (read-only)",
      "Rotated host",
      "sample.sqlite",
      "Shop MySQL",
      "Staging via SSH",
    ]);
    expect(order("tag")).toEqual([
      "Local Postgres",
      "sample.sqlite",
      "Shop MySQL",
      "Staging via SSH",
      "Production (read-only)",
      "Rotated host",
    ]);
    expect(order("driver")).toEqual([
      "Rotated host",
      "Shop MySQL",
      "Local Postgres",
      "Production (read-only)",
      "Staging via SSH",
      "sample.sqlite",
    ]);
  });

  it("sorts inside each group and shows saved empty groups unless searching", () => {
    expect(
      names(groupProfiles(SEED_PROFILES, "", { groups: ["Archive", "Local"], sort: "name" })),
    ).toEqual([
      ["", ["sample.sqlite"]],
      ["Archive", []],
      ["Local", ["Local Postgres", "Shop MySQL"]],
      ["Production", ["Production (read-only)"]],
      ["Staging", ["Rotated host", "Staging via SSH"]],
    ]);
    expect(names(groupProfiles(SEED_PROFILES, "shop", { groups: ["Archive"] }))).toEqual([
      ["Local", ["Shop MySQL"]],
    ]);
  });

  it("maps drop targets to groups", () => {
    expect(groupOfDropTarget(groupDropId("Staging"))).toBe("Staging");
    expect(groupOfDropTarget(groupDropId(""))).toBeNull();
    expect(groupOfDropTarget("p-local-pg")).toBeUndefined();
    expect(groupOfDropTarget(null)).toBeUndefined();
  });
});
