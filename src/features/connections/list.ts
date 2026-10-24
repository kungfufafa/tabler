import { isCoreDriver } from "@/features/drivers/catalog";
import type { ConnectionProfile, Driver } from "@/lib/ipc";
import { DRIVER_LABEL } from "./colors";
import {
  ancestorPaths,
  comparePaths,
  leafName,
  normalizePath,
  parentPath,
  pathDepth,
  splitPath,
} from "./groups";
import { connectionDetail } from "./schema";
import { compareTags } from "./tags";

/** DOM id of a profile's row in the launcher list (the list's `aria-activedescendant`). */
export const connectionRowId = (profileId: string) => `connection-row-${profileId}`;

/** "Sort by ▸" on the welcome screen; "manual" is the saved order (creation / reorder). */
export type ConnectionSort = "name" | "tag" | "driver" | "manual";
export const CONNECTION_SORTS = ["name", "tag", "driver", "manual"] as const;
export const CONNECTION_SORT_LABEL: Record<ConnectionSort, string> = {
  name: "Name",
  tag: "Tag",
  driver: "Driver",
  manual: "Manual",
};

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const byName = (a: ConnectionProfile, b: ConnectionProfile) => collator.compare(a.name, b.name);
const driverLabel = (driver: Driver) => (isCoreDriver(driver) ? DRIVER_LABEL[driver] : driver);

/** The connections of one group in the chosen order (stable: ties keep the saved order). */
export function sortProfiles(
  profiles: readonly ConnectionProfile[],
  sort: ConnectionSort,
): ConnectionProfile[] {
  const out = [...profiles];
  switch (sort) {
    case "manual":
      return out;
    case "name":
      return out.sort(byName);
    case "tag":
      return out.sort((a, b) => compareTags(a.options.tag, b.options.tag) || byName(a, b));
    case "driver":
      return out.sort(
        (a, b) => collator.compare(driverLabel(a.driver), driverLabel(b.driver)) || byName(a, b),
      );
  }
}

/** The canonical group path of a profile ("" for none). */
export const groupPathOf = (p: ConnectionProfile): string => normalizePath(p.group);

/** Whether a profile matches the search: name, host line, group path or tag. */
export function matchesQuery(p: ConnectionProfile, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return `${p.name} ${connectionDetail(p)} ${groupPathOf(p)} ${p.options.tag ?? ""}`
    .toLowerCase()
    .includes(q);
}

export interface GroupOptions {
  /** Saved group paths: shown (empty) even without connections, unless searching. */
  groups?: readonly string[];
  sort?: ConnectionSort;
}

/**
 * Search matches, grouped flat by group path: the connections without a group ("") first, then
 * the groups in tree order (a group right before its subgroups). The ungrouped connections are
 * shown without a header (like TablePlus).
 */
export function groupProfiles(
  profiles: readonly ConnectionProfile[],
  query: string,
  { groups: saved = [], sort = "manual" }: GroupOptions = {},
): Array<[string, ConnectionProfile[]]> {
  const searching = query.trim() !== "";
  const groups = new Map<string, ConnectionProfile[]>();
  if (!searching)
    for (const g of saved) {
      const path = normalizePath(g);
      if (path && !groups.has(path)) groups.set(path, []);
    }
  for (const p of profiles) {
    if (!matchesQuery(p, query)) continue;
    const key = groupPathOf(p);
    groups.set(key, [...(groups.get(key) ?? []), p]);
  }
  return [...groups.entries()]
    .map(([g, items]): [string, ConnectionProfile[]] => [g, sortProfiles(items, sort)])
    .sort(([a], [b]) => (a === "" ? -1 : b === "" ? 1 : comparePaths(a, b)));
}

/** One group of the welcome list's tree. */
export interface GroupNode {
  path: string;
  /** Its own name (the last level of the path). */
  name: string;
  /** 0 for a top-level group. */
  depth: number;
  /** Its own connections, in the chosen order. */
  profiles: ConnectionProfile[];
  /** Its subgroups, alphabetically. */
  children: GroupNode[];
  /** Connections in the group and all its subgroups. */
  count: number;
}

export interface ConnectionTree {
  /** The connections without a group (shown first, without a header). */
  ungrouped: ConnectionProfile[];
  /** The top-level groups, alphabetically. */
  groups: GroupNode[];
}

/**
 * The welcome list as a tree: every group (saved ones even when empty, and every enclosing group
 * of a path), each with its connections first, then its subgroups.
 */
export function buildConnectionTree(
  profiles: readonly ConnectionProfile[],
  { groups: saved = [], sort = "manual" }: GroupOptions = {},
): ConnectionTree {
  const nodes = new Map<string, GroupNode>();
  const node = (path: string): GroupNode => {
    let n = nodes.get(path);
    if (!n) {
      n = {
        path,
        name: leafName(path),
        depth: pathDepth(path),
        profiles: [],
        children: [],
        count: 0,
      };
      nodes.set(path, n);
      for (const a of ancestorPaths(path)) if (!nodes.has(a)) node(a);
    }
    return n;
  };
  const ungrouped: ConnectionProfile[] = [];
  for (const g of saved) {
    const path = normalizePath(g);
    if (path) node(path);
  }
  for (const p of profiles) {
    const path = groupPathOf(p);
    if (path) node(path).profiles.push(p);
    else ungrouped.push(p);
  }
  const top: GroupNode[] = [];
  for (const n of [...nodes.values()].sort((a, b) => comparePaths(a.path, b.path))) {
    n.profiles = sortProfiles(n.profiles, sort);
    if (n.depth === 0) top.push(n);
    else nodes.get(parentPath(n.path))?.children.push(n);
  }
  const count = (n: GroupNode): number => {
    n.count = n.profiles.length + n.children.reduce((sum, c) => sum + count(c), 0);
    return n.count;
  };
  top.forEach(count);
  return { ungrouped: sortProfiles(ungrouped, sort), groups: top };
}

/** Every group of the tree, in list order (pre-order). */
export function flattenGroups(nodes: readonly GroupNode[]): GroupNode[] {
  return nodes.flatMap((n) => [n, ...flattenGroups(n.children)]);
}

/**
 * Whether a group's rows are hidden. The ungrouped connections have no header, so they never
 * collapse (even if an older version stored them as collapsed); a search shows every match.
 */
export function isGroupCollapsed(
  group: string,
  collapsed: readonly string[],
  searching: boolean,
): boolean {
  return group !== "" && !searching && collapsed.includes(group);
}

/** The rows on screen, in list order (a collapsed group hides its connections and subgroups). */
export function visibleProfiles(
  tree: ConnectionTree,
  collapsed: readonly string[],
): ConnectionProfile[] {
  const walk = (n: GroupNode): ConnectionProfile[] =>
    collapsed.includes(n.path) ? [] : [...n.profiles, ...n.children.flatMap(walk)];
  return [...tree.ungrouped, ...tree.groups.flatMap(walk)];
}

/**
 * The matches of a search, flat, in list order (shown with their group path as a breadcrumb).
 */
export function searchProfiles(
  profiles: readonly ConnectionProfile[],
  query: string,
  sort: ConnectionSort = "manual",
): ConnectionProfile[] {
  const tree = buildConnectionTree(
    profiles.filter((p) => matchesQuery(p, query)),
    { sort },
  );
  return visibleProfiles(tree, []);
}

/** The levels of a profile's group, for its breadcrumb ([] without a group). */
export const breadcrumbOf = (p: ConnectionProfile): string[] => splitPath(p.group);

/** Drop target ids of the launcher's drag and drop: a group header, or "" for the top level. */
export const groupDropId = (group: string) => `group:${group}`;
/** Drag ids of group headers (connection rows use the profile id). */
export const groupDragId = (group: string) => `drag-group:${group}`;

/**
 * The group a connection (or group) dropped on `overId` moves to: the group's path, null for the
 * top level, undefined when it was not dropped on a group (nothing to do).
 */
export function groupOfDropTarget(
  overId: string | number | null | undefined,
): string | null | undefined {
  if (typeof overId !== "string" || !overId.startsWith("group:")) return undefined;
  const group = overId.slice("group:".length);
  return group === "" ? null : group;
}

/** The path of the group whose header is dragged; undefined for a connection. */
export function draggedGroup(activeId: string | number | null | undefined): string | undefined {
  if (typeof activeId !== "string" || !activeId.startsWith("drag-group:")) return undefined;
  return activeId.slice("drag-group:".length);
}
