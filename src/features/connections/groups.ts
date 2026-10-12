import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { toast } from "sonner";
import { ipc, toIpcError, type ConnectionProfile } from "@/lib/ipc";
import { profileKeys } from "./useConnections";

/**
 * Connection groups. A group is a name on profiles (`profile.group`); the names are also saved in
 * the settings key `connectionGroups`, so a group exists while it is empty (TablePlus's "New ▸
 * Group…").
 */

/** App DB settings key of the saved group names (mirrors `GROUPS_SETTING` in profiles.rs). */
export const GROUPS_SETTING = "connectionGroups";
export const groupKeys = { all: ["connection-groups"] as const };

/** Trimmed, non-empty, unique strings of a stored value (anything else is dropped). */
export function sanitizeGroups(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const v of raw) {
    if (typeof v !== "string") continue;
    const name = v.trim();
    if (name && !out.includes(name)) out.push(name);
  }
  return out;
}

export async function loadStoredGroups(): Promise<string[]> {
  return sanitizeGroups((await ipc.settings.get())[GROUPS_SETTING]);
}

export function useStoredGroups() {
  return useQuery({ queryKey: groupKeys.all, queryFn: loadStoredGroups, staleTime: Infinity });
}

const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const same = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

/** Every group: the saved names and those used by profiles, alphabetically. */
export function knownGroups(
  stored: readonly string[],
  profiles: readonly ConnectionProfile[],
): string[] {
  const names = sanitizeGroups([...stored, ...profiles.map((p) => p.group ?? "")]);
  return names.sort((a, b) => collator.compare(a, b));
}

export function addGroupName(list: readonly string[], name: string): string[] {
  const n = name.trim();
  return !n || list.some((g) => g === n) ? [...list] : [...list, n];
}

export function renameGroupName(list: readonly string[], from: string, to: string): string[] {
  const t = to.trim();
  const out = list.filter((g) => g !== from && g !== t);
  return t ? [...out, t] : out;
}

export function removeGroupName(list: readonly string[], name: string): string[] {
  return list.filter((g) => g !== name);
}

/** Why `name` cannot be used for a new (or renamed `current`) group; null when it can. */
export function groupNameError(
  name: string,
  existing: readonly string[],
  current: string | null = null,
): string | null {
  const n = name.trim();
  if (!n) return "Enter a group name";
  if (n.length > 60) return "Use at most 60 characters";
  if (existing.some((g) => same(g, n) && g !== current)) return `“${n}” already exists`;
  return null;
}

/** The ids of the profiles in `group`. */
export function idsInGroup(profiles: readonly ConnectionProfile[], group: string): string[] {
  return profiles.filter((p) => (p.group?.trim() || "") === group).map((p) => p.id);
}

async function storedGroups(qc: QueryClient): Promise<string[]> {
  return qc.ensureQueryData({ queryKey: groupKeys.all, queryFn: loadStoredGroups });
}

async function persistGroups(qc: QueryClient, next: string[]): Promise<void> {
  await ipc.settings.set(GROUPS_SETTING, next);
  qc.setQueryData(groupKeys.all, next);
}

export interface GroupActions {
  create: (name: string) => Promise<boolean>;
  rename: (from: string, to: string) => Promise<boolean>;
  /** Deletes the group; its connections move out of it (to no group). */
  remove: (name: string) => Promise<boolean>;
  /** Moves profiles into `group` (null: no group). */
  moveTo: (ids: string[], group: string | null) => Promise<boolean>;
}

/** Group operations; each resolves false (after a toast) when it failed. */
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
    return {
      create: (name) =>
        attempt("Could not create the group", async () => {
          await persistGroups(qc, addGroupName(await storedGroups(qc), name));
        }),
      rename: (from, to) =>
        attempt("Could not rename the group", async () => {
          const ids = idsInGroup(profiles(), from);
          if (ids.length > 0) await ipc.profiles.moveToGroup(ids, to.trim());
          await persistGroups(qc, renameGroupName(await storedGroups(qc), from, to));
        }),
      remove: (name) =>
        attempt("Could not delete the group", async () => {
          const ids = idsInGroup(profiles(), name);
          if (ids.length > 0) await ipc.profiles.moveToGroup(ids, null);
          await persistGroups(qc, removeGroupName(await storedGroups(qc), name));
        }),
      moveTo: (ids, group) =>
        attempt("Could not move the connection", async () => {
          await ipc.profiles.moveToGroup(ids, group);
          if (group) await persistGroups(qc, addGroupName(await storedGroups(qc), group));
        }),
    };
  }, [qc]);
}
