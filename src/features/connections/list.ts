import type { ConnectionProfile } from "@/lib/ipc";
import { connectionDetail } from "./schema";

/** DOM id of a profile's row in the launcher list (the list's `aria-activedescendant`). */
export const connectionRowId = (profileId: string) => `connection-row-${profileId}`;

/**
 * Search matches, grouped: the connections without a group ("") first, then the groups
 * alphabetically. The ungrouped connections are shown without a header (like TablePlus).
 */
export function groupProfiles(
  profiles: readonly ConnectionProfile[],
  query: string,
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
  for (const p of filtered) {
    const key = p.group?.trim() || "";
    groups.set(key, [...(groups.get(key) ?? []), p]);
  }
  return [...groups.entries()].sort(([a], [b]) =>
    a === "" ? -1 : b === "" ? 1 : a.localeCompare(b),
  );
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
