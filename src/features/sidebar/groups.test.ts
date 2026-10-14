import { beforeEach, describe, expect, it } from "vitest";
import type { RoutineSummary, TableSummary } from "@/lib/ipc";
import {
  addToGroup,
  createGroup,
  deleteGroup,
  editGroup,
  groupOf,
  GROUPS_STORAGE_KEY,
  groupWarning,
  groupsOf,
  inGroup,
  kindOfTable,
  parseGroups,
  regexError,
  reloadGroups,
  removeFromGroup,
  routineItem,
  ruleMatches,
  setGroupCollapsed,
  tableItem,
  type GroupItem,
  type SidebarGroup,
} from "./groups";

function table(name: string, kind: TableSummary["kind"] = "table", schema = "public") {
  return tableItem({ schema, name, kind });
}

const routine: RoutineSummary = {
  schema: "public",
  name: "log_rotate",
  kind: "procedure",
  arguments: "",
  identityArguments: "",
  returnType: null,
  language: "plpgsql",
  comment: null,
};

const group = (patch: Partial<SidebarGroup>): SidebarGroup => ({
  id: "g",
  name: "G",
  rules: [],
  members: [],
  collapsed: false,
  ...patch,
});

const SCOPE = "p1|db";

describe("sidebar group rules", () => {
  it("matches names by prefix, suffix, substring (ignoring case) and regex", () => {
    const item = table("Log_2024_01");
    expect(ruleMatches({ field: "name", op: "prefix", value: "log_" }, item)).toBe(true);
    expect(ruleMatches({ field: "name", op: "prefix", value: "2024" }, item)).toBe(false);
    expect(ruleMatches({ field: "name", op: "suffix", value: "_01" }, item)).toBe(true);
    expect(ruleMatches({ field: "name", op: "contains", value: "2024" }, item)).toBe(true);
    expect(ruleMatches({ field: "name", op: "regex", value: "^Log_\\d{4}_\\d+$" }, item)).toBe(
      true,
    );
    // A regex is used as written (case-sensitive); an invalid one matches nothing.
    expect(ruleMatches({ field: "name", op: "regex", value: "^log_" }, item)).toBe(false);
    expect(ruleMatches({ field: "name", op: "regex", value: "(" }, item)).toBe(false);
    expect(regexError("(")).toMatch(/./);
    expect(regexError("^a|b$")).toBeNull();
  });

  it("refuses regexes that could hang the sidebar", { timeout: 2_000 }, () => {
    for (const unsafe of [
      "(\\w+)+x$",
      "(a*)*",
      "(?:x{2,})+b",
      "^((ab)+)*$",
      "(a|b+)*",
      "(a+)+?",
      "x".repeat(201),
    ])
      expect(regexError(unsafe), unsafe).toMatch(/./);
    for (const safe of [
      "^log_\\d+$",
      "^(log|audit)_\\d+$",
      "(\\d+_)?x",
      "(?:ab)+",
      "[(+)]+",
      "(a\\+)+",
      "(?<y>\\d{4})_\\d+",
      "(?<=a)\\w+",
      "x".repeat(200),
    ])
      expect(regexError(safe), safe).toBeNull();
    // A catastrophic rule (stored by an older version) matches nothing, at once.
    const evil = { field: "name", op: "regex", value: "(\\w+)+x$" } as const;
    expect(ruleMatches(evil, table("a".repeat(40)))).toBe(false);
    // Only the start of a very long name is tested.
    const b = { field: "name", op: "regex", value: "b$" } as const;
    expect(ruleMatches(b, table(`${"a".repeat(200)}b`))).toBe(false);
    expect(ruleMatches(b, table("ab"))).toBe(true);
  });

  it("matches kinds and schemas", () => {
    expect(kindOfTable("partitioned")).toBe("table");
    expect(kindOfTable("foreignTable")).toBe("table");
    expect(kindOfTable("systemView")).toBe("view");
    expect(ruleMatches({ field: "kind", value: "view" }, table("v", "view"))).toBe(true);
    expect(ruleMatches({ field: "kind", value: "view" }, table("m", "materializedView"))).toBe(
      false,
    );
    expect(ruleMatches({ field: "kind", value: "function" }, routineItem(routine))).toBe(true);
    expect(ruleMatches({ field: "schema", value: "audit" }, table("t", "table", "audit"))).toBe(
      true,
    );
    expect(ruleMatches({ field: "schema", value: "audit" }, table("t"))).toBe(false);
  });

  it("needs every rule to match (AND); a manual group holds its members", () => {
    const logs = group({
      rules: [
        { field: "name", op: "prefix", value: "log_" },
        { field: "kind", value: "table" },
      ],
    });
    expect(inGroup(logs, table("log_a"))).toBe(true);
    expect(inGroup(logs, table("log_v", "view"))).toBe(false);
    expect(inGroup(group({ rules: [] }), table("x"))).toBe(false);
    const manual = group({ rules: null, members: ["t:public.users"] });
    expect(inGroup(manual, table("users"))).toBe(true);
    expect(inGroup(manual, table("users", "table", "audit"))).toBe(false);
    expect(routineItem(routine).key).toBe("f:public.log_rotate()");
  });

  it("lists an item in the first group that holds it", () => {
    const a = group({ id: "a", rules: [{ field: "name", op: "contains", value: "log" }] });
    const b = group({ id: "b", rules: null, members: ["t:public.log_x"] });
    const item: GroupItem = table("log_x");
    expect(groupOf([a, b], item)?.id).toBe("a");
    expect(groupOf([b, a], item)?.id).toBe("b");
    expect(groupOf([b], table("other"))).toBeUndefined();
  });
});

describe("sidebar groups store", () => {
  beforeEach(() => {
    localStorage.clear();
    reloadGroups();
  });

  it("keeps the well-formed part of a stored value", () => {
    expect(parseGroups("x")).toEqual({});
    expect(
      parseGroups({
        s: [
          { id: "a", name: "A", rules: [{ field: "kind", value: "view" }, { field: "bad" }] },
          { id: "", name: "nameless" },
          { id: "b", name: "B", rules: null, members: ["t:x", 3, "t:x"], collapsed: true },
        ],
        empty: [],
      }),
    ).toEqual({
      s: [
        {
          id: "a",
          name: "A",
          rules: [{ field: "kind", value: "view" }],
          members: [],
          collapsed: false,
        },
        { id: "b", name: "B", rules: null, members: ["t:x"], collapsed: true },
      ],
    });
  });

  it("switches off a stored group whose regex is unsafe, with a warning", () => {
    localStorage.setItem(
      GROUPS_STORAGE_KEY,
      JSON.stringify({
        [SCOPE]: [
          {
            id: "evil",
            name: "Evil",
            rules: [
              { field: "kind", value: "table" },
              { field: "name", op: "regex", value: "(\\w+)+x$" },
            ],
          },
          { id: "ok", name: "Logs", rules: [{ field: "name", op: "regex", value: "^log_" }] },
        ],
      }),
    );
    reloadGroups();
    const [evil, ok] = groupsOf(SCOPE);
    // Kept as stored (Edit Group… shows it), but it holds nothing: not every table.
    expect(evil?.rules).toHaveLength(2);
    expect(inGroup(evil!, table("users"))).toBe(false);
    expect(inGroup(evil!, table("a".repeat(60)))).toBe(false);
    expect(groupWarning(evil!)).toMatch(/off/);
    expect(inGroup(ok!, table("log_a"))).toBe(true);
    expect(groupWarning(ok!)).toBeNull();
  });

  it("creates, edits, collapses and deletes groups, persisted per scope", () => {
    const g = createGroup(SCOPE, "  Views ", [{ field: "kind", value: "view" }]);
    expect(groupsOf(SCOPE)).toEqual([
      {
        id: g.id,
        name: "Views",
        rules: [{ field: "kind", value: "view" }],
        members: [],
        collapsed: false,
      },
    ]);
    expect(groupsOf("other|db")).toEqual([]);
    editGroup(SCOPE, g.id, "All views", null);
    setGroupCollapsed(SCOPE, g.id, true);
    reloadGroups();
    expect(groupsOf(SCOPE)[0]).toMatchObject({ name: "All views", rules: null, collapsed: true });
    expect(JSON.parse(localStorage.getItem(GROUPS_STORAGE_KEY)!)[SCOPE]).toHaveLength(1);
    deleteGroup(SCOPE, g.id);
    expect(groupsOf(SCOPE)).toEqual([]);
    expect(JSON.parse(localStorage.getItem(GROUPS_STORAGE_KEY)!)).toEqual({});
  });

  it("moves items between manual groups (an item is in one of them)", () => {
    const a = createGroup(SCOPE, "A", null, ["t:public.users"]);
    const b = createGroup(SCOPE, "B", null);
    const rules = createGroup(SCOPE, "R", [{ field: "kind", value: "view" }]);
    addToGroup(SCOPE, b.id, ["t:public.users", "f:public.add(a integer)"]);
    const [ga, gb] = groupsOf(SCOPE);
    expect(ga!.members).toEqual([]);
    expect(gb!.members).toEqual(["t:public.users", "f:public.add(a integer)"]);
    // Rule groups take no members.
    addToGroup(SCOPE, rules.id, ["t:public.posts"]);
    expect(groupsOf(SCOPE)[2]!.members).toEqual([]);
    removeFromGroup(SCOPE, b.id, ["t:public.users"]);
    expect(groupsOf(SCOPE)[1]!.members).toEqual(["f:public.add(a integer)"]);
    expect(a.id).not.toBe(b.id);
  });
});
