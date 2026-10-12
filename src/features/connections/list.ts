import type { ConnectionProfile, ConnectionTag } from "@/lib/ipc";
import { DRIVER_LABEL } from "./colors";
import { CONNECTION_TAGS, connectionDetail } from "./schema";

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
const tagRank = (tag: ConnectionTag | null) =>
  tag === null ? CONNECTION_TAGS.length : CONNECTION_TAGS.indexOf(tag);

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
      return out.sort((a, b) => tagRank(a.options.tag) - tagRank(b.options.tag) || byName(a, b));
    case "driver":
      return out.sort(
        (a, b) => collator.compare(DRIVER_LABEL[a.driver], DRIVER_LABEL[b.driver]) || byName(a, b),
      );
  }
}

export interface GroupOptions {
  /** Saved group names: shown (empty) even without connections, unless searching. */
  groups?: readonly string[];
  sort?: ConnectionSort;
}

/**
 * Search matches, grouped: the connections without a group ("") first, then the groups
 * alphabetically. The ungrouped connections are shown without a header (like TablePlus).
 */
export function groupProfiles(
  profiles: readonly ConnectionProfile[],
  query: string,
  { groups: saved = [], sort = "manual" }: GroupOptions = {},
): Array<[string, ConnectionProfile[]]> {
  const q = query.trim().toLowerCase();
  const filtered = q
    ? profiles.filter((p) =>
        `${p.name} ${connectionDetail(p)} ${p.group ?? ""} ${p.options.tag ?? ""}`
          .toLowerCase()
          .includes(q),
      )
    : profiles;
  const groups = new Map<string, ConnectionProfile[]>();
  if (!q)
    for (const g of saved) {
      const name = g.trim();
      if (name && !groups.has(name)) groups.set(name, []);
    }
  for (const p of filtered) {
    const key = p.group?.trim() || "";
    groups.set(key, [...(groups.get(key) ?? []), p]);
  }
  return [...groups.entries()]
    .map(([g, items]): [string, ConnectionProfile[]] => [g, sortProfiles(items, sort)])
    .sort(([a], [b]) => (a === "" ? -1 : b === "" ? 1 : collator.compare(a, b)));
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

/** The rows on screen, in list order (rows of collapsed groups left out). */
export function visibleProfiles(
  groups: ReadonlyArray<readonly [string, readonly ConnectionProfile[]]>,
  collapsed: readonly string[],
  searching: boolean,
): ConnectionProfile[] {
  return groups.flatMap(([group, items]) =>
    isGroupCollapsed(group, collapsed, searching) ? [] : items,
  );
}

/** Drop target ids of the launcher's drag and drop: a group header, or "no group". */
export const groupDropId = (group: string) => `group:${group}`;

/**
 * The group a connection dropped on `overId` moves to: the group's name, null for "no group",
 * undefined when it was not dropped on a group (nothing to do).
 */
export function groupOfDropTarget(
  overId: string | number | null | undefined,
): string | null | undefined {
  if (typeof overId !== "string" || !overId.startsWith("group:")) return undefined;
  const group = overId.slice("group:".length);
  return group === "" ? null : group;
}
