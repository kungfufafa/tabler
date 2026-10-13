import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import {
  Archive,
  Box,
  Briefcase,
  Building2,
  Cloud,
  Code,
  Cpu,
  Database,
  Flame,
  FlaskConical,
  Folder,
  GitBranch,
  Globe,
  HardDrive,
  House,
  Layers,
  Lock,
  Rocket,
  Server,
  Shield,
  Star,
  Users,
  Wrench,
  Zap,
  type LucideIcon,
} from "lucide-react";
import { useMemo } from "react";
import { toast } from "sonner";
import { ipc, toIpcError, type ConnectionProfile } from "@/lib/ipc";
import { profileKeys } from "./useConnections";

/**
 * Connection groups form a tree. A profile's `group` is a path whose levels are joined by
 * " / " (`Clients / Acme / Prod`); the groups themselves (with their icons, and empty ones too:
 * TablePlus's "New ▸ Group…") are saved in the settings key `connectionGroups` as
 * `{ path, icon }` records. Older versions saved plain names: they read as top-level groups.
 */

/** App DB settings key of the saved groups (mirrors `GROUPS_SETTING` in profiles.rs). */
export const GROUPS_SETTING = "connectionGroups";
export const groupKeys = { all: ["connection-groups"] as const };

/** Between the levels of a group path. */
export const GROUP_SEPARATOR = " / ";
/** A "/" with white space on either side separates levels, so `CI/CD` stays one name. */
const SEPARATOR_RE = /\s+\/\s*|\s*\/\s+/;
/** Longest group path: the connection form's Group field takes at most 60 characters. */
export const MAX_GROUP_PATH = 200;

// ---- icons ---------------------------------------------------------------------------------

/**
 * The icons a group can show on its header. TablePlus lets a group carry any image; Tabler
 * offers this curated set of icons instead (they follow the theme in light and dark).
 */
export const GROUP_ICONS = {
  folder: { label: "Folder", icon: Folder },
  server: { label: "Server", icon: Server },
  database: { label: "Database", icon: Database },
  cloud: { label: "Cloud", icon: Cloud },
  "hard-drive": { label: "Disk", icon: HardDrive },
  cpu: { label: "Chip", icon: Cpu },
  building: { label: "Building", icon: Building2 },
  briefcase: { label: "Briefcase", icon: Briefcase },
  users: { label: "Team", icon: Users },
  home: { label: "Home", icon: House },
  globe: { label: "Globe", icon: Globe },
  shield: { label: "Shield", icon: Shield },
  lock: { label: "Lock", icon: Lock },
  flask: { label: "Lab", icon: FlaskConical },
  rocket: { label: "Rocket", icon: Rocket },
  code: { label: "Code", icon: Code },
  "git-branch": { label: "Branch", icon: GitBranch },
  wrench: { label: "Tools", icon: Wrench },
  layers: { label: "Layers", icon: Layers },
  box: { label: "Box", icon: Box },
  archive: { label: "Archive", icon: Archive },
  star: { label: "Star", icon: Star },
  zap: { label: "Lightning", icon: Zap },
  flame: { label: "Flame", icon: Flame },
} as const satisfies Record<string, { label: string; icon: LucideIcon }>;

export type GroupIconId = keyof typeof GROUP_ICONS;
export const GROUP_ICON_IDS = Object.keys(GROUP_ICONS) as GroupIconId[];

export const isGroupIconId = (v: unknown): v is GroupIconId =>
  typeof v === "string" && Object.hasOwn(GROUP_ICONS, v);

// ---- paths ---------------------------------------------------------------------------------

/** The levels of a group path (trimmed, empty ones dropped); [] for no group. */
export function splitPath(path: string | null | undefined): string[] {
  return (path ?? "")
    .split(SEPARATOR_RE)
    .map((l) => l.trim())
    .filter(Boolean);
}

export const joinPath = (levels: readonly string[]): string => levels.join(GROUP_SEPARATOR);

/** A group path in its canonical form ("" for no group). Mirrors `normalize_group_path`. */
export const normalizePath = (raw: string | null | undefined): string => joinPath(splitPath(raw));

/** The enclosing group's path; "" for a top-level group. */
export const parentPath = (path: string): string => joinPath(splitPath(path).slice(0, -1));

/** The group's own name (its last level). */
export const leafName = (path: string): string => splitPath(path).at(-1) ?? "";

/** 0 for a top-level group, 1 for its subgroups… (-1 for no group). */
export const pathDepth = (path: string): number => splitPath(path).length - 1;

export const childPath = (parent: string, name: string): string =>
  joinPath([...splitPath(parent), ...splitPath(name)]);

/** The enclosing groups of `path`, outermost first (`path` itself left out). */
export function ancestorPaths(path: string): string[] {
  const levels = splitPath(path);
  return levels.slice(0, -1).map((_, i) => joinPath(levels.slice(0, i + 1)));
}

/** Whether `path` is `root` or one of its subgroups (canonical paths). */
export const isWithin = (path: string, root: string): boolean =>
  root !== "" && (path === root || path.startsWith(root + GROUP_SEPARATOR));

/** `path` after its group `from` became `to` (renamed or moved, subgroups along). */
export const rebasePath = (path: string, from: string, to: string): string =>
  isWithin(path, from) ? to + path.slice(from.length) : path;

/**
 * `path` after the group `removed` was deleted: its subgroups move up one level; `removed`
 * itself is gone (null).
 */
export function liftPath(path: string, removed: string): string | null {
  if (path === removed) return null;
  return isWithin(path, removed)
    ? childPath(parentPath(removed), path.slice(removed.length))
    : path;
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const same = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

/** Tree order: level by level, alphabetically; a group comes right before its subgroups. */
export function comparePaths(a: string, b: string): number {
  const x = splitPath(a);
  const y = splitPath(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const c = collator.compare(x[i]!, y[i]!);
    if (c !== 0) return c;
  }
  return x.length - y.length;
}

// ---- saved groups --------------------------------------------------------------------------

export interface GroupRecord {
  /** Canonical path (`Clients / Acme`). */
  path: string;
  /** null: no icon on the header. */
  icon: GroupIconId | null;
}

/**
 * The saved groups of a stored value: `{ path, icon }` records, or the plain names older
 * versions saved (migrated as top-level groups without an icon). Paths are made canonical and
 * kept once; unknown icons are dropped; anything else is skipped.
 */
export function sanitizeGroups(raw: unknown): GroupRecord[] {
  if (!Array.isArray(raw)) return [];
  const out: GroupRecord[] = [];
  for (const v of raw) {
    const rec =
      typeof v === "string"
        ? { path: v, icon: null }
        : typeof v === "object" && v !== null && typeof (v as { path?: unknown }).path === "string"
          ? (v as { path: string; icon?: unknown })
          : null;
    if (!rec) continue;
    const icon = isGroupIconId(rec.icon) ? rec.icon : null;
    addGroup(out, rec.path, icon);
  }
  return out;
}

/** Adds `path` to `list` in place (a group already there without an icon takes `icon`). */
function addGroup(list: GroupRecord[], raw: string, icon: GroupIconId | null = null): void {
  const path = normalizePath(raw);
  if (!path) return;
  const existing = list.find((g) => g.path === path);
  if (!existing) list.push({ path, icon });
  else if (!existing.icon) existing.icon = icon;
}

export async function loadStoredGroups(): Promise<GroupRecord[]> {
  return sanitizeGroups((await ipc.settings.get())[GROUPS_SETTING]);
}

export function useStoredGroups() {
  return useQuery({ queryKey: groupKeys.all, queryFn: loadStoredGroups, staleTime: Infinity });
}

/**
 * Every group path, in tree order: the saved groups, those profiles use, and all their
 * enclosing groups.
 */
export function knownGroups(
  stored: readonly GroupRecord[],
  profiles: readonly ConnectionProfile[],
): string[] {
  const paths = new Set<string>();
  for (const raw of [...stored.map((g) => g.path), ...profiles.map((p) => p.group)]) {
    const path = normalizePath(raw);
    if (!path) continue;
    for (const a of ancestorPaths(path)) paths.add(a);
    paths.add(path);
  }
  return [...paths].sort(comparePaths);
}

/** The icon saved for `path` (null when none). */
export const iconOf = (stored: readonly GroupRecord[], path: string): GroupIconId | null =>
  stored.find((g) => g.path === path)?.icon ?? null;

/** Why `name` cannot be used for a group in `parent`; null when it can. */
export function groupNameError(
  name: string,
  /** The names of the other groups in the same parent. */
  siblings: readonly string[],
  /** The group being renamed (its current name), null for a new one. */
  current: string | null = null,
  parent = "",
): string | null {
  const n = name.trim();
  if (!n) return "Enter a group name";
  if (splitPath(n).length !== 1 || n !== splitPath(n)[0])
    return "A group name cannot contain “ / ”: use New Subgroup… to nest groups";
  if (childPath(parent, n).length > MAX_GROUP_PATH)
    return `Use at most ${MAX_GROUP_PATH} characters for the whole group path`;
  if (siblings.some((g) => same(g, n) && g !== current))
    return parent ? `“${n}” already exists in “${parent}”` : `“${n}” already exists`;
  return null;
}

/** The names of the groups directly inside `parent` ("" = top level). */
export const childNames = (paths: readonly string[], parent: string): string[] =>
  paths.filter((p) => p !== "" && parentPath(p) === parent).map(leafName);

/**
 * Why the group `path` cannot move into `newParent` ("" = top level); null when it can (or it
 * already is there).
 */
export function moveGroupError(
  path: string,
  newParent: string,
  paths: readonly string[],
): string | null {
  if (parentPath(path) === newParent) return null;
  if (isWithin(newParent, path)) return "A group cannot move into itself or one of its subgroups";
  const target = childPath(newParent, leafName(path));
  if (paths.some((p) => same(p, target)))
    return newParent
      ? `“${leafName(path)}” already exists in “${newParent}”`
      : `“${leafName(path)}” already exists at the top level`;
  const longest = paths
    .filter((p) => isWithin(p, path))
    .map((p) => rebasePath(p, path, target).length);
  if (Math.max(target.length, ...longest) > MAX_GROUP_PATH)
    return `The group paths would get longer than ${MAX_GROUP_PATH} characters`;
  return null;
}

/** The ids of the profiles directly in `group` ("" = no group). */
export function idsInGroup(profiles: readonly ConnectionProfile[], group: string): string[] {
  const g = normalizePath(group);
  return profiles.filter((p) => normalizePath(p.group) === g).map((p) => p.id);
}

/** The ids of the profiles in `group` and its subgroups. */
export function idsInSubtree(profiles: readonly ConnectionProfile[], group: string): string[] {
  const g = normalizePath(group);
  return profiles.filter((p) => isWithin(normalizePath(p.group), g)).map((p) => p.id);
}

// ---- operations ----------------------------------------------------------------------------

/** What a group operation does: the saved groups afterwards and the profiles to move. */
export interface GroupPlan {
  groups: GroupRecord[];
  moves: Array<{ ids: string[]; group: string | null }>;
}

function remap(
  records: readonly GroupRecord[],
  profiles: readonly ConnectionProfile[],
  group: (path: string) => string | null,
  profile: (path: string) => string,
): GroupPlan {
  const groups: GroupRecord[] = [];
  for (const r of records) {
    const path = group(r.path);
    if (path !== null) addGroup(groups, path, r.icon);
  }
  const targets = new Map<string, string[]>();
  for (const p of profiles) {
    const from = normalizePath(p.group);
    if (!from) continue;
    const to = profile(from);
    if (to !== from) targets.set(to, [...(targets.get(to) ?? []), p.id]);
  }
  const moves = [...targets].map(([g, ids]) => ({ ids, group: g === "" ? null : g }));
  return { groups, moves };
}

/** Renaming or moving the group `from` to the path `to`: subgroups and connections follow. */
export function planRenameGroup(
  records: readonly GroupRecord[],
  profiles: readonly ConnectionProfile[],
  from: string,
  to: string,
): GroupPlan {
  const move = (p: string) => rebasePath(p, from, to);
  const plan = remap(records, profiles, move, move);
  // A group only profiles used (never saved) is saved under its new path, so it stays put.
  addGroup(plan.groups, to);
  return plan;
}

/** Deleting `path`: its connections and subgroups move up one level. */
export function planDeleteGroup(
  records: readonly GroupRecord[],
  profiles: readonly ConnectionProfile[],
  path: string,
): GroupPlan {
  return remap(
    records,
    profiles,
    (p) => liftPath(p, path),
    (p) => (p === path ? parentPath(path) : (liftPath(p, path) ?? "")),
  );
}

/** The saved groups with `icon` on `path` (saving the group if it was only used). */
export function withIcon(
  records: readonly GroupRecord[],
  path: string,
  icon: GroupIconId | null,
): GroupRecord[] {
  const p = normalizePath(path);
  const out = records.map((g) => (g.path === p ? { ...g, icon } : g));
  if (p && !out.some((g) => g.path === p)) out.push({ path: p, icon });
  return out;
}

async function storedGroups(qc: QueryClient): Promise<GroupRecord[]> {
  return qc.ensureQueryData({ queryKey: groupKeys.all, queryFn: loadStoredGroups });
}

async function persistGroups(qc: QueryClient, next: GroupRecord[]): Promise<void> {
  await ipc.settings.set(GROUPS_SETTING, next);
  qc.setQueryData(groupKeys.all, next);
}

/**
 * Carries out a rename / delete `plan` all or nothing: the profile moves first, then the saved
 * groups. When a step fails, the profiles moved so far go back to the groups they were in
 * (`profiles`, as they were when the plan was made) before the error is passed on, so the tree is
 * never left half renamed. The caller remaps its collapsed groups only once this resolved.
 */
export async function applyGroupPlan(
  qc: QueryClient,
  plan: GroupPlan,
  profiles: readonly ConnectionProfile[],
): Promise<void> {
  const tried: string[] = [];
  try {
    for (const m of plan.moves) {
      // Counted before the call: a failed move may have moved some of its profiles.
      tried.push(...m.ids);
      await ipc.profiles.moveToGroup(m.ids, m.group);
    }
    await persistGroups(qc, plan.groups);
  } catch (err) {
    const restored = await moveBack(tried, profiles);
    if (restored) throw err;
    throw new Error(
      `${toIpcError(err).message}. Some connections could not be moved back to their groups.`,
      { cause: err },
    );
  }
}

/** Puts `ids` back into the groups they had in `profiles`; false when a move failed. */
async function moveBack(ids: readonly string[], profiles: readonly ConnectionProfile[]) {
  // The group exactly as it was stored (not made canonical): back is back.
  const was = new Map(profiles.map((p) => [p.id, p.group]));
  const back = new Map<string | null, string[]>();
  for (const id of ids) {
    const group = was.get(id);
    if (group !== undefined) back.set(group, [...(back.get(group) ?? []), id]);
  }
  let ok = true;
  for (const [group, list] of back) {
    try {
      await ipc.profiles.moveToGroup(list, group);
    } catch {
      ok = false;
    }
  }
  return ok;
}

export interface GroupActions {
  /** Saves the group `path` (subgroups: a path inside another group). */
  create: (path: string, icon?: GroupIconId | null) => Promise<boolean>;
  /** Renames (or moves) the group to the path `to`; subgroups and connections follow. */
  rename: (from: string, to: string) => Promise<boolean>;
  /** Moves the group into `newParent` ("" = top level). */
  move: (path: string, newParent: string) => Promise<boolean>;
  /** Deletes the group; its connections and subgroups move up one level. */
  remove: (path: string) => Promise<boolean>;
  setIcon: (path: string, icon: GroupIconId | null) => Promise<boolean>;
  /** Moves profiles into `group` (null: no group). */
  moveTo: (ids: string[], group: string | null) => Promise<boolean>;
}

/**
 * Group operations; each resolves false (after a toast) when it failed. The launcher's
 * collapsed groups are the caller's to follow (`useLauncherPrefs().renameCollapsed`).
 */
export function useGroupActions(): GroupActions {
  const qc = useQueryClient();
  return useMemo(() => {
    const profiles = () => qc.getQueryData<ConnectionProfile[]>(profileKeys.all) ?? [];
    const refresh = () => qc.invalidateQueries({ queryKey: profileKeys.all });
    const attempt = async (what: string, run: () => Promise<void>) => {
      try {
        await run();
        return true;
      } catch (err) {
        toast.error(what, { description: toIpcError(err).message });
        return false;
      } finally {
        void refresh();
      }
    };
    const rename = (from: string, to: string) =>
      attempt("Could not rename the group", async () => {
        const [f, t] = [normalizePath(from), normalizePath(to)];
        if (!f || !t || f === t) return;
        const before = profiles();
        await applyGroupPlan(qc, planRenameGroup(await storedGroups(qc), before, f, t), before);
      });
    return {
      create: (path, icon = null) =>
        attempt("Could not create the group", async () => {
          const next = [...(await storedGroups(qc))];
          addGroup(next, path, icon);
          await persistGroups(qc, next);
        }),
      rename,
      move: (path, newParent) => rename(path, childPath(newParent, leafName(path))),
      remove: (path) =>
        attempt("Could not delete the group", async () => {
          const p = normalizePath(path);
          const before = profiles();
          await applyGroupPlan(qc, planDeleteGroup(await storedGroups(qc), before, p), before);
        }),
      setIcon: (path, icon) =>
        attempt("Could not change the icon", async () => {
          await persistGroups(qc, withIcon(await storedGroups(qc), path, icon));
        }),
      moveTo: (ids, group) =>
        attempt("Could not move the connection", async () => {
          const g = normalizePath(group);
          await ipc.profiles.moveToGroup(ids, g || null);
          if (g) {
            const next = [...(await storedGroups(qc))];
            addGroup(next, g);
            await persistGroups(qc, next);
          }
        }),
    };
  }, [qc]);
}
