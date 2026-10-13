import { create } from "zustand";
import { tableKey, type RoutineSummary, type TableKind, type TableRef } from "@/lib/ipc";

/**
 * Sidebar groups (TablePlus b446, spec §2.3): named, collapsible sections above Tables. A group
 * with filter rules collects every item matching all of its rules (e.g. partitions by name prefix,
 * or the views); one without rules is a manual folder that items are dragged (or "Add to Group"-ed)
 * into. Grouped items leave the regular sections. Groups belong to a saved connection and its
 * database (`pinScope`), persisted in localStorage like the other sidebar preferences.
 */

/** What "Kind is …" can match; every routine (function, procedure…) counts as "function". */
export type GroupKind = "table" | "view" | "materializedView" | "function";

export type NameOp = "prefix" | "suffix" | "contains" | "regex";

export type GroupRule =
  | { field: "name"; op: NameOp; value: string }
  | { field: "kind"; value: GroupKind }
  | { field: "schema"; value: string };

export interface SidebarGroup {
  id: string;
  name: string;
  /** Filter rules, all of which must match; null for a manual group. */
  rules: GroupRule[] | null;
  /** Manual groups: the item keys put in it (`groupItemKey`). */
  members: string[];
  collapsed: boolean;
}

/** An item a group can hold. */
export interface GroupItem {
  /** `groupItemKey`. */
  key: string;
  name: string;
  schema: string | null;
  kind: GroupKind;
}

/** localStorage key of the groups (per `pinScope`). */
export const GROUPS_STORAGE_KEY = "tabler.sidebar.groups";

export const GROUP_KINDS: ReadonlyArray<[GroupKind, string]> = [
  ["table", "table"],
  ["view", "view"],
  ["materializedView", "materialized view"],
  ["function", "function"],
];

export const NAME_OPS: ReadonlyArray<[NameOp, string]> = [
  ["prefix", "has prefix"],
  ["suffix", "has suffix"],
  ["contains", "contains"],
  ["regex", "matches regex"],
];

export const EMPTY_GROUPS: readonly SidebarGroup[] = Object.freeze([]);

/** The group kind of a table-like object (system views count as views). */
export function kindOfTable(kind: TableKind): GroupKind {
  switch (kind) {
    case "view":
    case "systemView":
      return "view";
    case "materializedView":
      return "materializedView";
    default:
      return "table";
  }
}

/**
 * Item keys: `t:<schema>.<name>` for tables and views, `f:<schema>.<name>(<args>)` routines,
 * `s:<schema>.<name>` sequences (not groupable; the key of their sidebar row).
 */
export const tableItemKey = (t: TableRef): string => `t:${tableKey(t)}`;
export const routineItemKey = (
  r: Pick<RoutineSummary, "schema" | "name" | "identityArguments">,
): string => `f:${r.schema ?? ""}.${r.name}(${r.identityArguments})`;
export const sequenceItemKey = (s: TableRef): string => `s:${tableKey(s)}`;

export function tableItem(t: TableRef & { kind: TableKind }): GroupItem {
  return { key: tableItemKey(t), name: t.name, schema: t.schema, kind: kindOfTable(t.kind) };
}

export function routineItem(r: RoutineSummary): GroupItem {
  return { key: routineItemKey(r), name: r.name, schema: r.schema, kind: "function" };
}

/** Longest "matches regex" pattern a rule may use. */
export const MAX_REGEX_LENGTH = 200;

/** A regex rule tests at most this many characters of a name (identifiers are far shorter). */
export const MAX_TESTED_NAME = 128;

/** The length of the quantifier (`*`, `+`, `?`, `{2}`, `{2,}`, `{2,5}`) at `i` in `source`; 0 when none. */
function quantifierAt(source: string, i: number): number {
  const ch = source[i];
  if (ch === "*" || ch === "+" || ch === "?") return 1;
  if (ch !== "{") return 0;
  return /^\{\d+(?:,\d*)?\}/.exec(source.slice(i, i + 24))?.[0].length ?? 0;
}

/**
 * Whether `source` repeats a group that itself holds a repetition (`(\w+)+`, `(a*)*`,
 * `(?:x{2,})+`): the shape behind catastrophic backtracking, which would freeze the sidebar on
 * every item. A heuristic for a valid pattern, not a proof: overlapping alternatives (`(a|a)*`)
 * pass. An optional group (`(\d+_)?`) is fine.
 */
export function hasNestedQuantifier(source: string): boolean {
  /** Per open group: whether it holds a quantifier. */
  const open: boolean[] = [];
  /** The atom just before: a group holding a quantifier (true), another group (false), else null. */
  let closed: boolean | null = null;
  let inClass = false;
  for (let i = 0; i < source.length; i++) {
    const ch = source[i]!;
    if (ch === "\\") {
      i++;
      if (!inClass) closed = null;
      continue;
    }
    if (inClass) {
      if (ch === "]") inClass = false;
      continue;
    }
    if (ch === "[") {
      inClass = true;
      closed = null;
      continue;
    }
    if (ch === "(") {
      open.push(false);
      closed = null;
      // `(?:`, `(?=`, `(?!`, `(?<=`, `(?<!`, `(?<name>`: their `?` is no quantifier.
      if (source[i + 1] === "?") {
        i += 1;
        if (source[i + 1] !== "<") i += 1;
        else if (source[i + 2] === "=" || source[i + 2] === "!") i += 2;
        else {
          const end = source.indexOf(">", i);
          i = end < 0 ? source.length : end;
        }
      }
      continue;
    }
    if (ch === ")") {
      closed = open.pop() ?? false;
      if (closed && open.length > 0) open[open.length - 1] = true;
      continue;
    }
    const q = quantifierAt(source, i);
    if (q === 0) {
      closed = null;
      continue;
    }
    if (closed === true && ch !== "?") return true;
    if (open.length > 0) open[open.length - 1] = true;
    i += q - 1;
    // A lazy quantifier's `?`.
    if (source[i + 1] === "?") i++;
    closed = null;
  }
  return false;
}

/** Why a "matches regex" value cannot be used, or null. */
export function regexError(value: string): string | null {
  if (value.length > MAX_REGEX_LENGTH)
    return `use at most ${MAX_REGEX_LENGTH} characters (this one has ${value.length})`;
  try {
    new RegExp(value);
  } catch (e) {
    return e instanceof Error ? e.message : "Invalid regular expression";
  }
  return hasNestedQuantifier(value)
    ? "a repeated group that repeats inside, like (a+)+, can take forever to match"
    : null;
}

/** Compiled "matches regex" rules by pattern (null: unusable), so each compiles once. */
const compiled = new Map<string, RegExp | null>();

function ruleRegex(source: string): RegExp | null {
  let re = compiled.get(source);
  if (re === undefined) {
    re = regexError(source) === null ? new RegExp(source) : null;
    if (compiled.size >= 256) compiled.clear();
    compiled.set(source, re);
  }
  return re;
}

/**
 * Whether `item` matches `rule`. Name prefix / suffix / contains ignore case; a regex is used as
 * written on the first `MAX_TESTED_NAME` characters (one `regexError` refuses matches nothing);
 * Schema is an exact name.
 */
export function ruleMatches(rule: GroupRule, item: GroupItem): boolean {
  switch (rule.field) {
    case "kind":
      return item.kind === rule.value;
    case "schema":
      return (item.schema ?? "") === rule.value;
    case "name": {
      const name = item.name.toLowerCase();
      const value = rule.value.toLowerCase();
      switch (rule.op) {
        case "prefix":
          return name.startsWith(value);
        case "suffix":
          return name.endsWith(value);
        case "contains":
          return name.includes(value);
        case "regex":
          return ruleRegex(rule.value)?.test(item.name.slice(0, MAX_TESTED_NAME)) ?? false;
      }
    }
  }
}

/**
 * Why `group` is switched off, or null: a stored "matches regex" rule that is invalid or unsafe
 * (saved by an older version, or edited by hand) matches nothing until Edit Group… fixes it.
 */
export function groupWarning(group: SidebarGroup): string | null {
  for (const rule of group.rules ?? []) {
    if (rule.field !== "name" || rule.op !== "regex") continue;
    const err = regexError(rule.value);
    if (err)
      return `This group is off: its regex cannot be used (${err}). Edit the group to fix it.`;
  }
  return null;
}

/** Whether `item` belongs to `group`: every rule matches (a rule group without rules holds
 * nothing), or a manual group lists it. */
export function inGroup(group: SidebarGroup, item: GroupItem): boolean {
  if (group.rules === null) return group.members.includes(item.key);
  return group.rules.length > 0 && group.rules.every((r) => ruleMatches(r, item));
}

/** The first group (in sidebar order) holding `item`; an item is listed in one group only. */
export function groupOf(
  groups: readonly SidebarGroup[],
  item: GroupItem,
): SidebarGroup | undefined {
  return groups.find((g) => inGroup(g, item));
}

/** Keeps the well-formed rules of a stored value. */
function parseRule(value: unknown): GroupRule | null {
  if (!value || typeof value !== "object") return null;
  const r = value as Record<string, unknown>;
  if (typeof r.value !== "string") return null;
  if (r.field === "kind")
    return GROUP_KINDS.some(([k]) => k === r.value)
      ? { field: "kind", value: r.value as GroupKind }
      : null;
  if (r.field === "schema") return { field: "schema", value: r.value };
  if (r.field === "name" && NAME_OPS.some(([op]) => op === r.op))
    return { field: "name", op: r.op as NameOp, value: r.value };
  return null;
}

function parseGroup(value: unknown): SidebarGroup | null {
  if (!value || typeof value !== "object") return null;
  const g = value as Record<string, unknown>;
  if (typeof g.id !== "string" || !g.id || typeof g.name !== "string" || !g.name.trim())
    return null;
  const rules = Array.isArray(g.rules)
    ? g.rules.map(parseRule).filter((r): r is GroupRule => r !== null)
    : null;
  const members = Array.isArray(g.members)
    ? [...new Set(g.members.filter((m): m is string => typeof m === "string" && m !== ""))]
    : [];
  return { id: g.id, name: g.name, rules, members, collapsed: g.collapsed === true };
}

/** Keeps the well-formed part of a stored value: group lists under scope keys. */
export function parseGroups(value: unknown): Record<string, SidebarGroup[]> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, SidebarGroup[]> = {};
  for (const [scope, list] of Object.entries(value as Record<string, unknown>)) {
    if (!Array.isArray(list)) continue;
    const groups = list.map(parseGroup).filter((g): g is SidebarGroup => g !== null);
    if (groups.length > 0) out[scope] = groups;
  }
  return out;
}

function readStored(): Record<string, SidebarGroup[]> {
  try {
    const raw = localStorage.getItem(GROUPS_STORAGE_KEY);
    return raw ? parseGroups(JSON.parse(raw)) : {};
  } catch {
    // Unavailable storage (private window, blocked site data) or a malformed value.
    return {};
  }
}

function writeStored(groups: Record<string, SidebarGroup[]>): void {
  try {
    localStorage.setItem(GROUPS_STORAGE_KEY, JSON.stringify(groups));
  } catch {
    // Not persisted; the in-memory groups still work for this session.
  }
}

interface GroupsState {
  groups: Record<string, SidebarGroup[]>;
}

export const useGroupsStore = create<GroupsState>()(() => ({ groups: readStored() }));

function update(scope: string, fn: (list: SidebarGroup[]) => SidebarGroup[]): void {
  const all = useGroupsStore.getState().groups;
  const current = all[scope] ?? [];
  const next = fn(current);
  if (next === current) return;
  const groups = { ...all };
  if (next.length === 0) delete groups[scope];
  else groups[scope] = next;
  useGroupsStore.setState({ groups });
  writeStored(groups);
}

function patchGroup(scope: string, id: string, patch: (g: SidebarGroup) => SidebarGroup): void {
  update(scope, (list) => {
    const i = list.findIndex((g) => g.id === id);
    if (i < 0) return list;
    const next = patch(list[i]!);
    if (next === list[i]) return list;
    const out = [...list];
    out[i] = next;
    return out;
  });
}

export function groupsOf(scope: string | null): readonly SidebarGroup[] {
  return (scope !== null ? useGroupsStore.getState().groups[scope] : undefined) ?? EMPTY_GROUPS;
}

/** The groups of `scope` in sidebar order (a stable reference until they change). */
export function useGroups(scope: string | null): readonly SidebarGroup[] {
  return useGroupsStore((s) => (scope !== null ? s.groups[scope] : undefined) ?? EMPTY_GROUPS);
}

/** Adds a group at the end; `rules` null makes a manual group. */
export function createGroup(
  scope: string,
  name: string,
  rules: GroupRule[] | null,
  members: string[] = [],
): SidebarGroup {
  const group: SidebarGroup = {
    id: crypto.randomUUID(),
    name: name.trim(),
    rules,
    members: [...new Set(members)],
    collapsed: false,
  };
  update(scope, (list) => [...list, group]);
  return group;
}

/** Edit Group…: new name and rules (members are kept for when the rules are turned off again). */
export function editGroup(
  scope: string,
  id: string,
  name: string,
  rules: GroupRule[] | null,
): void {
  patchGroup(scope, id, (g) => ({ ...g, name: name.trim(), rules }));
}

export function deleteGroup(scope: string, id: string): void {
  update(scope, (list) => (list.some((g) => g.id === id) ? list.filter((g) => g.id !== id) : list));
}

/**
 * Puts items into a manual group. An item lives in one manual group: it leaves the others of the
 * scope.
 */
export function addToGroup(scope: string, id: string, keys: readonly string[]): void {
  if (keys.length === 0) return;
  const moving = new Set(keys);
  update(scope, (list) => {
    const target = list.find((g) => g.id === id);
    if (!target || target.rules !== null) return list;
    return list.map((g) => {
      if (g.id === id) return { ...g, members: [...new Set([...g.members, ...keys])] };
      if (g.rules !== null || !g.members.some((m) => moving.has(m))) return g;
      return { ...g, members: g.members.filter((m) => !moving.has(m)) };
    });
  });
}

export function removeFromGroup(scope: string, id: string, keys: readonly string[]): void {
  const leaving = new Set(keys);
  patchGroup(scope, id, (g) =>
    g.members.some((m) => leaving.has(m))
      ? { ...g, members: g.members.filter((m) => !leaving.has(m)) }
      : g,
  );
}

export function setGroupCollapsed(scope: string, id: string, collapsed: boolean): void {
  patchGroup(scope, id, (g) => (g.collapsed === collapsed ? g : { ...g, collapsed }));
}

/** Test helper: reads the stored groups again. */
export function reloadGroups(): void {
  useGroupsStore.setState({ groups: readStored() });
}

// --- the New Group… / Edit Group… dialog ----------------------------------------------------------

export interface GroupDialogRequest {
  scope: string;
  /** The group being edited; absent for New Group…. */
  group?: SidebarGroup;
  /** Items to put in a new manual group (Add to Group ▸ New Group…). */
  members?: string[];
}

interface GroupDialogState {
  request: GroupDialogRequest | null;
}

export const useGroupDialogStore = create<GroupDialogState>()(() => ({ request: null }));

export function openGroupDialog(request: GroupDialogRequest): void {
  useGroupDialogStore.setState({ request });
}

export function closeGroupDialog(): void {
  useGroupDialogStore.setState({ request: null });
}
