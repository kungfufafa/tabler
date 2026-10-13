import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import {
  ancestorPaths,
  applyGroupPlan,
  childNames,
  childPath,
  comparePaths,
  groupKeys,
  groupNameError,
  iconOf,
  idsInGroup,
  idsInSubtree,
  isWithin,
  knownGroups,
  leafName,
  liftPath,
  moveGroupError,
  normalizePath,
  parentPath,
  pathDepth,
  planDeleteGroup,
  planRenameGroup,
  rebasePath,
  sanitizeGroups,
  splitPath,
  withIcon,
  type GroupRecord,
} from "./groups";
import { remapCollapsed } from "./store";

const pg = SEED_PROFILES[0]!;
const at = (id: string, group: string | null): ConnectionProfile => ({ ...pg, id, group });
/** A small tree: Clients / Acme / Prod, Clients / Acme / Staging, Clients / Globex, Local. */
const PROFILES = [
  at("acme-prod", "Clients / Acme / Prod"),
  at("acme-stg", "Clients/ Acme /Staging"),
  at("acme", "Clients / Acme"),
  at("globex", "Clients / Globex"),
  at("local", "Local"),
  at("loose", null),
];
const rec = (path: string, icon: GroupRecord["icon"] = null): GroupRecord => ({ path, icon });

describe("group paths", () => {
  it("splits on “ / ” only, trimming levels and dropping empty ones", () => {
    expect(splitPath(" Clients / Acme /Prod ")).toEqual(["Clients", "Acme", "Prod"]);
    expect(normalizePath("Clients/ Acme")).toBe("Clients / Acme");
    // A slash inside a word belongs to the name.
    expect(normalizePath("CI/CD")).toBe("CI/CD");
    expect(normalizePath("A / / B / ")).toBe("A / B");
    expect(normalizePath(null)).toBe("");
  });

  it("knows a path's parent, name, depth and ancestors", () => {
    const path = "Clients / Acme / Prod";
    expect(parentPath(path)).toBe("Clients / Acme");
    expect(parentPath("Clients")).toBe("");
    expect(leafName(path)).toBe("Prod");
    expect(pathDepth(path)).toBe(2);
    expect(pathDepth("Clients")).toBe(0);
    expect(ancestorPaths(path)).toEqual(["Clients", "Clients / Acme"]);
    expect(childPath("", "Local")).toBe("Local");
    expect(childPath("Clients", "Acme")).toBe("Clients / Acme");
  });

  it("tells subgroups apart from groups that only share a prefix", () => {
    expect(isWithin("Clients / Acme", "Clients")).toBe(true);
    expect(isWithin("Clients", "Clients")).toBe(true);
    expect(isWithin("Clientsville", "Clients")).toBe(false);
    expect(isWithin("Clients", "")).toBe(false);
  });

  it("renaming or moving a group moves its descendants; deleting lifts them one level", () => {
    expect(rebasePath("Clients / Acme / Prod", "Clients / Acme", "Customers / Acme")).toBe(
      "Customers / Acme / Prod",
    );
    expect(rebasePath("Clients / Acmeish", "Clients / Acme", "X")).toBe("Clients / Acmeish");
    expect(liftPath("Clients / Acme / Prod", "Clients / Acme")).toBe("Clients / Prod");
    expect(liftPath("Clients / Acme", "Clients")).toBe("Acme");
    expect(liftPath("Clients", "Clients")).toBeNull();
    expect(liftPath("Local", "Clients")).toBe("Local");
  });

  it("orders paths as a tree: a group right before its subgroups", () => {
    const paths = ["Local", "Clients / Globex", "Clients", "Clients / Acme / Prod", "clients 2"];
    expect([...paths].sort(comparePaths)).toEqual([
      "Clients",
      "Clients / Acme / Prod",
      "Clients / Globex",
      "clients 2",
      "Local",
    ]);
  });
});

describe("saved groups", () => {
  it("migrates the old list of names and reads records defensively", () => {
    expect(sanitizeGroups(["A", " A ", "", 3, null, "B"])).toEqual([rec("A"), rec("B")]);
    expect(
      sanitizeGroups([
        { path: "Clients /Acme", icon: "building" },
        { path: "Clients / Acme", icon: "rocket" },
        { path: "Odd", icon: "no-such-icon" },
        { icon: "server" },
        "Clients / Acme",
      ]),
    ).toEqual([rec("Clients / Acme", "building"), rec("Odd")]);
    expect(sanitizeGroups("A")).toEqual([]);
    expect(sanitizeGroups(undefined)).toEqual([]);
  });

  it("lists saved, used and enclosing groups once, in tree order", () => {
    expect(knownGroups([rec("Archive"), rec("Local")], SEED_PROFILES)).toEqual([
      "Archive",
      "Local",
      "Production",
      "Staging",
    ]);
    expect(knownGroups([rec("Clients / Initech / Empty")], PROFILES)).toEqual([
      "Clients",
      "Clients / Acme",
      "Clients / Acme / Prod",
      "Clients / Acme / Staging",
      "Clients / Globex",
      "Clients / Initech",
      "Clients / Initech / Empty",
      "Local",
    ]);
    expect(childNames(knownGroups([], PROFILES), "Clients")).toEqual(["Acme", "Globex"]);
    expect(childNames(knownGroups([], PROFILES), "")).toEqual(["Clients", "Local"]);
  });

  it("validates names against the sibling groups, ignoring case", () => {
    expect(groupNameError(" ", [])).toBe("Enter a group name");
    expect(groupNameError("staging", ["Staging"])).toBe("“staging” already exists");
    expect(groupNameError("staging", ["Staging"], null, "Clients")).toBe(
      "“staging” already exists in “Clients”",
    );
    expect(groupNameError("Staging", ["Staging"], "Staging")).toBeNull();
    expect(groupNameError("x".repeat(201), [])).toMatch(/200/);
    expect(groupNameError("x".repeat(190), [], null, "Clients / Acme")).toMatch(/200/);
    expect(groupNameError("Acme / Prod", [])).toMatch(/New Subgroup/);
    expect(groupNameError("CI/CD", [])).toBeNull();
    expect(groupNameError("Archive", ["Staging"])).toBeNull();
  });

  it("finds the profiles of a group, or of its whole subtree", () => {
    expect(idsInGroup(SEED_PROFILES, "Staging")).toEqual(["p-staging-ssh", "p-rotated-ssh"]);
    expect(idsInGroup(SEED_PROFILES, "")).toEqual(["p-sample-sqlite"]);
    expect(idsInGroup(SEED_PROFILES, "Nope")).toEqual([]);
    expect(idsInGroup(PROFILES, "Clients / Acme / Staging")).toEqual(["acme-stg"]);
    expect(idsInSubtree(PROFILES, "Clients / Acme")).toEqual(["acme-prod", "acme-stg", "acme"]);
    expect(idsInSubtree(PROFILES, "")).toEqual([]);
  });
});

describe("group operations", () => {
  const all = knownGroups([], PROFILES);

  it("refuses moving a group into its own subtree or onto a taken name", () => {
    expect(moveGroupError("Clients / Acme", "Clients / Acme", all)).toMatch(/itself/);
    expect(moveGroupError("Clients", "Clients / Acme / Prod", all)).toMatch(/itself/);
    expect(moveGroupError("Clients / Acme", "", all)).toBeNull();
    expect(moveGroupError("Clients / Acme", "Local", all)).toBeNull();
    // Already there: nothing to refuse (and nothing to do).
    expect(moveGroupError("Clients / Acme", "Clients", all)).toBeNull();
    expect(moveGroupError("Local", "Clients", [...all, "Clients / local"])).toBe(
      "“Local” already exists in “Clients”",
    );
    expect(moveGroupError("Clients / Globex", "", [...all, "Globex"])).toMatch(/top level/);
    expect(moveGroupError("Local", "x".repeat(195), all)).toMatch(/200/);
  });

  it("a rename moves the subgroups and connections along, keeping icons", () => {
    const records = [rec("Clients", "briefcase"), rec("Clients / Acme / Prod", "rocket")];
    const plan = planRenameGroup(records, PROFILES, "Clients", "Customers");
    expect(plan.groups).toEqual([
      rec("Customers", "briefcase"),
      rec("Customers / Acme / Prod", "rocket"),
    ]);
    expect(plan.moves).toEqual([
      { ids: ["acme-prod"], group: "Customers / Acme / Prod" },
      { ids: ["acme-stg"], group: "Customers / Acme / Staging" },
      { ids: ["acme"], group: "Customers / Acme" },
      { ids: ["globex"], group: "Customers / Globex" },
    ]);
    // A group only connections used is saved under its new path.
    const moved = planRenameGroup([], PROFILES, "Clients / Acme", "Local / Acme");
    expect(moved.groups).toEqual([rec("Local / Acme")]);
    expect(moved.moves.map((m) => m.group)).toEqual([
      "Local / Acme / Prod",
      "Local / Acme / Staging",
      "Local / Acme",
    ]);
  });

  it("deleting a group moves its connections and subgroups up one level", () => {
    const records = [rec("Clients"), rec("Clients / Acme", "building"), rec("Local")];
    const plan = planDeleteGroup(records, PROFILES, "Clients / Acme");
    expect(plan.groups).toEqual([rec("Clients"), rec("Local")]);
    expect(plan.moves).toEqual([
      { ids: ["acme-prod"], group: "Clients / Prod" },
      { ids: ["acme-stg"], group: "Clients / Staging" },
      { ids: ["acme"], group: "Clients" },
    ]);
    // A top-level group's own connections end up without a group.
    expect(planDeleteGroup([rec("Local")], PROFILES, "Local")).toEqual({
      groups: [],
      moves: [{ ids: ["local"], group: null }],
    });
  });

  it("collapsed groups follow renames and moves; deleting lifts the subgroups' state", () => {
    expect(remapCollapsed(["Clients", "Clients / Acme", "Local"], "Clients", "Customers")).toEqual([
      "Customers",
      "Customers / Acme",
      "Local",
    ]);
    expect(remapCollapsed(["Clients", "Clients / Acme / Prod"], "Clients / Acme", null)).toEqual([
      "Clients",
      "Clients / Prod",
    ]);
    expect(remapCollapsed(["A", "B"], "A", "B")).toEqual(["B"]);
    expect(remapCollapsed(["Local"], "Clients", null)).toEqual(["Local"]);
  });

  it("sets an icon, saving a group that was only used", () => {
    expect(withIcon([rec("A")], "A", "star")).toEqual([rec("A", "star")]);
    expect(withIcon([rec("A", "star")], "B / C", "cloud")).toEqual([
      rec("A", "star"),
      rec("B / C", "cloud"),
    ]);
    expect(iconOf(withIcon([rec("A", "star")], "A", null), "A")).toBeNull();
  });
});

describe("applying a group plan", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** A query client holding the saved groups, and the IPC calls the plan makes. */
  function setup(fail: (group: string | null) => boolean, saveFails = false) {
    const qc = new QueryClient();
    qc.setQueryData(groupKeys.all, [rec("Clients", "briefcase")]);
    const moves: Array<[string[], string | null]> = [];
    vi.spyOn(ipc.profiles, "moveToGroup").mockImplementation(async (ids, group) => {
      moves.push([ids, group]);
      if (fail(group)) throw new Error("disk full");
      return [];
    });
    const save = vi.spyOn(ipc.settings, "set").mockImplementation(async () => {
      if (saveFails) throw new Error("settings locked");
    });
    return { qc, moves, save };
  }
  const rename = () =>
    planRenameGroup([rec("Clients", "briefcase")], PROFILES, "Clients", "Customers");

  it("moves the connections, then saves the groups", async () => {
    const { qc, moves, save } = setup(() => false);
    await applyGroupPlan(qc, rename(), PROFILES);
    expect(moves.map(([, g]) => g)).toEqual([
      "Customers / Acme / Prod",
      "Customers / Acme / Staging",
      "Customers / Acme",
      "Customers / Globex",
    ]);
    expect(save).toHaveBeenCalledWith("connectionGroups", [rec("Customers", "briefcase")]);
    expect(qc.getQueryData(groupKeys.all)).toEqual([rec("Customers", "briefcase")]);
  });

  it("moves the connections back when one move fails, and saves nothing", async () => {
    const { qc, moves, save } = setup((g) => g === "Customers / Acme / Staging");
    await expect(applyGroupPlan(qc, rename(), PROFILES)).rejects.toThrow("disk full");
    expect(moves).toEqual([
      [["acme-prod"], "Customers / Acme / Prod"],
      [["acme-stg"], "Customers / Acme / Staging"],
      // Back where they were (the failed move may have half happened), as they were stored.
      [["acme-prod"], "Clients / Acme / Prod"],
      [["acme-stg"], "Clients/ Acme /Staging"],
    ]);
    expect(save).not.toHaveBeenCalled();
    expect(qc.getQueryData(groupKeys.all)).toEqual([rec("Clients", "briefcase")]);
  });

  it("moves every connection back when the groups cannot be saved", async () => {
    const { qc, moves } = setup(() => false, true);
    const plan = planDeleteGroup([rec("Local")], PROFILES, "Local");
    await expect(applyGroupPlan(qc, plan, PROFILES)).rejects.toThrow("settings locked");
    expect(moves).toEqual([
      [["local"], null],
      [["local"], "Local"],
    ]);
    expect(qc.getQueryData(groupKeys.all)).toEqual([rec("Clients", "briefcase")]);
  });

  it("says so when the connections could not be moved back either", async () => {
    const { qc } = setup((g) => g !== "Customers / Acme / Prod");
    await expect(applyGroupPlan(qc, rename(), PROFILES)).rejects.toThrow(
      "disk full. Some connections could not be moved back to their groups.",
    );
  });
});
