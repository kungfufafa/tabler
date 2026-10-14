import { describe, expect, it } from "vitest";
import type { ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import {
  breadcrumbOf,
  buildConnectionTree,
  draggedGroup,
  flattenGroups,
  groupDragId,
  groupDropId,
  groupOfDropTarget,
  groupProfiles,
  isGroupCollapsed,
  searchProfiles,
  sortProfiles,
  visibleProfiles,
  type GroupNode,
} from "./list";

const names = (groups: ReturnType<typeof groupProfiles>) =>
  groups.map(([g, items]) => [g, items.map((p) => p.name)]);

const pg = SEED_PROFILES[0]!;
const at = (id: string, group: string | null): ConnectionProfile => ({
  ...pg,
  id,
  name: id,
  group,
});
const NESTED = [
  at("acme-prod", "Clients / Acme / Prod"),
  at("acme", "Clients / Acme"),
  at("globex", "Clients /Globex"),
  at("loose", null),
  at("local", "Local"),
];
/** Each group as `path (count): own connection ids`, in list order. */
const outline = (nodes: readonly GroupNode[]) =>
  flattenGroups(nodes).map(
    (n) =>
      `${"  ".repeat(n.depth)}${n.name} (${n.count}): ${n.profiles.map((p) => p.id).join(",")}`,
  );

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

describe("the connection tree", () => {
  it("nests groups by path, counting the connections of the subgroups too", () => {
    const tree = buildConnectionTree(NESTED, { groups: ["Clients / Initech", "Archive"] });
    expect(tree.ungrouped.map((p) => p.id)).toEqual(["loose"]);
    expect(outline(tree.groups)).toEqual([
      "Archive (0): ",
      "Clients (3): ",
      "  Acme (2): acme",
      "    Prod (1): acme-prod",
      "  Globex (1): globex",
      "  Initech (0): ",
      "Local (1): local",
    ]);
    const clients = tree.groups[1]!;
    expect(clients.children.map((c) => c.path)).toEqual([
      "Clients / Acme",
      "Clients / Globex",
      "Clients / Initech",
    ]);
  });

  it("works on the flat seed groups as before", () => {
    const tree = buildConnectionTree(SEED_PROFILES);
    expect(outline(tree.groups)).toEqual([
      "Local (2): p-local-pg,p-shop-mysql",
      "Production (1): p-prod-pg",
      "Staging (2): p-staging-ssh,p-rotated-ssh",
    ]);
  });
});

describe("visibleProfiles", () => {
  const ids = (rows: readonly ConnectionProfile[]) => rows.map((p) => p.id);
  const seed = buildConnectionTree(SEED_PROFILES);
  const nested = buildConnectionTree(NESTED);

  it("leaves out the rows of collapsed groups, keeping the list order", () => {
    expect(ids(visibleProfiles(seed, ["Local", "Staging"]))).toEqual([
      "p-sample-sqlite",
      "p-prod-pg",
    ]);
    expect(ids(visibleProfiles(seed, []))).toHaveLength(SEED_PROFILES.length);
  });

  it("a group's connections come before its subgroups; collapsing hides the whole subtree", () => {
    expect(ids(visibleProfiles(nested, []))).toEqual([
      "loose",
      "acme",
      "acme-prod",
      "globex",
      "local",
    ]);
    expect(ids(visibleProfiles(nested, ["Clients / Acme"]))).toEqual(["loose", "globex", "local"]);
    expect(ids(visibleProfiles(nested, ["Clients"]))).toEqual(["loose", "local"]);
  });

  it("never hides the ungrouped connections (they have no header to expand them again)", () => {
    expect(isGroupCollapsed("", [""], false)).toBe(false);
    expect(ids(visibleProfiles(seed, ["", "Local", "Production", "Staging"]))).toEqual([
      "p-sample-sqlite",
    ]);
  });

  it("search lists every match flat in list order, ignoring collapsed groups", () => {
    expect(isGroupCollapsed("Staging", ["Staging"], true)).toBe(false);
    expect(ids(searchProfiles(SEED_PROFILES, "staging"))).toEqual([
      "p-staging-ssh",
      "p-rotated-ssh",
    ]);
    // Group paths are searched, so a parent's name finds the connections of its subgroups.
    expect(ids(searchProfiles(NESTED, "clients"))).toEqual(["acme", "acme-prod", "globex"]);
    expect(breadcrumbOf(NESTED[0]!)).toEqual(["Clients", "Acme", "Prod"]);
    expect(breadcrumbOf(NESTED[3]!)).toEqual([]);
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

  it("groups nested paths flat in tree order (the backup window's list)", () => {
    expect(names(groupProfiles(NESTED, ""))).toEqual([
      ["", ["loose"]],
      ["Clients / Acme", ["acme"]],
      ["Clients / Acme / Prod", ["acme-prod"]],
      ["Clients / Globex", ["globex"]],
      ["Local", ["local"]],
    ]);
  });

  it("maps drop targets to groups and drag ids to dragged groups", () => {
    expect(groupOfDropTarget(groupDropId("Staging"))).toBe("Staging");
    expect(groupOfDropTarget(groupDropId("Clients / Acme"))).toBe("Clients / Acme");
    expect(groupOfDropTarget(groupDropId(""))).toBeNull();
    expect(groupOfDropTarget("p-local-pg")).toBeUndefined();
    expect(groupOfDropTarget(null)).toBeUndefined();
    expect(draggedGroup(groupDragId("Clients / Acme"))).toBe("Clients / Acme");
    expect(draggedGroup("p-local-pg")).toBeUndefined();
    // A dragged header is never mistaken for a drop target, nor the other way round.
    expect(groupOfDropTarget(groupDragId("Staging"))).toBeUndefined();
    expect(draggedGroup(groupDropId("Staging"))).toBeUndefined();
  });
});
