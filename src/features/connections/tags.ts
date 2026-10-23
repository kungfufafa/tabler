import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { create } from "zustand";
import { ipc, type ConnectionProfile, type ConnectionTag } from "@/lib/ipc";
import { ACCENT_PRESETS, type AccentPreset } from "@/lib/theme";
import { HEX_COLOR, TAG_COLOR, TAG_LABEL } from "./colors";
import { CONNECTION_TAGS, TAG_NAME_MAX, type BuiltinTag } from "./schema";
import { profileKeys } from "./useConnections";

/**
 * Connection tags: TablePlus's five built-in environment tags plus custom tags (b494) with a
 * name and a colour. A profile stores its tag's name (`options.tag`); the custom tags themselves
 * (name + colour) are saved in the settings key `connectionTags`. A profile whose custom tag was
 * deleted keeps the name, shown in gray.
 */

export interface CustomTag {
  name: string;
  /** An accent preset name or a `#rrggbb` colour. */
  color: string;
}

export { TAG_COLOR, TAG_LABEL };
export const TAGS_SETTING = "connectionTags";

/** The colour of a tag nobody defines (a deleted custom tag, one from an imported file). */
export const UNKNOWN_TAG_COLOR: AccentPreset = "gray";

export function isBuiltinTag(tag: string): tag is BuiltinTag {
  return (CONNECTION_TAGS as readonly string[]).includes(tag);
}

/** A colour a custom tag may have. */
export function isTagColor(color: unknown): color is string {
  return (
    typeof color === "string" &&
    (HEX_COLOR.test(color) || (ACCENT_PRESETS as readonly string[]).includes(color))
  );
}

/** Valid, unique (case-insensitive) custom tags that are not built-in names, in their order. */
export function sanitizeTags(raw: unknown): CustomTag[] {
  if (!Array.isArray(raw)) return [];
  const out: CustomTag[] = [];
  const seen = new Set<string>();
  for (const item of raw) {
    if (item === null || typeof item !== "object") continue;
    const { name, color } = item as Record<string, unknown>;
    if (typeof name !== "string") continue;
    const n = name.trim();
    const key = n.toLowerCase();
    if (!n || n.length > TAG_NAME_MAX || isBuiltinTag(key) || seen.has(key)) continue;
    seen.add(key);
    out.push({ name: n, color: isTagColor(color) ? color : UNKNOWN_TAG_COLOR });
  }
  return out;
}

/** What the Edit Tags dialog may save: an error message, or null. */
export function tagsError(tags: readonly CustomTag[]): string | null {
  const seen = new Set<string>();
  for (const t of tags) {
    const n = t.name.trim();
    if (!n) return "Every tag needs a name";
    if (n.length > TAG_NAME_MAX) return `Use at most ${TAG_NAME_MAX} characters for a tag name`;
    const key = n.toLowerCase();
    if (isBuiltinTag(key)) return `“${n}” is a built-in tag`;
    if (seen.has(key)) return `There are two tags named “${n}”`;
    seen.add(key);
  }
  return null;
}

/**
 * The custom tags, loaded once from the settings (Edit Tags… updates them). A store rather than a
 * query, so the tag pills work anywhere (and the lookups outside React see the same list).
 */
export const useCustomTagsStore = create<{ tags: readonly CustomTag[]; loaded: boolean }>()(() => ({
  tags: [],
  loaded: false,
}));

let loading: Promise<CustomTag[]> | null = null;

export async function loadCustomTags(): Promise<CustomTag[]> {
  loading ??= ipc.settings
    .get()
    .then((all) => {
      const tags = sanitizeTags(all[TAGS_SETTING]);
      useCustomTagsStore.setState({ tags, loaded: true });
      return tags;
    })
    .catch(() => {
      loading = null;
      return [];
    });
  return loading;
}

/** Test helper: forget the loaded tags. */
export function resetCustomTags(tags: readonly CustomTag[] = [], loaded = false): void {
  loading = loaded ? Promise.resolve([...tags]) : null;
  useCustomTagsStore.setState({ tags, loaded });
}

/** The custom tags (loads them on first use). */
export function useCustomTags(): readonly CustomTag[] {
  const tags = useCustomTagsStore((s) => s.tags);
  const loaded = useCustomTagsStore((s) => s.loaded);
  useEffect(() => {
    if (!loaded) void loadCustomTags();
  }, [loaded]);
  return tags;
}

/** The custom tag named `tag` (case-insensitive). */
export function customTag(
  tag: string,
  tags: readonly CustomTag[] = useCustomTagsStore.getState().tags,
): CustomTag | undefined {
  const key = tag.toLowerCase();
  return tags.find((t) => t.name.toLowerCase() === key);
}

/** Display name: "Production" for a built-in, the name for a custom tag. */
export function tagLabel(tag: ConnectionTag): string {
  return isBuiltinTag(tag) ? TAG_LABEL[tag] : tag;
}

const TAG_SHORT: Record<BuiltinTag, string | null> = {
  local: null,
  testing: "TEST",
  development: "DEV",
  staging: "STG",
  production: "PROD",
};

/**
 * The few letters a workspace icon shows for a tag ("PROD", a custom tag's first four), or null
 * for `local` and no tag: a badge only where the environment matters.
 */
export function tagShortLabel(tag: ConnectionTag | null | undefined): string | null {
  if (!tag) return null;
  if (isBuiltinTag(tag)) return TAG_SHORT[tag];
  return tag.slice(0, 4).toUpperCase();
}

/** The tag's colour: a preset name or a hex colour. */
export function tagColorOf(
  tag: ConnectionTag,
  tags: readonly CustomTag[] = useCustomTagsStore.getState().tags,
): string {
  if (isBuiltinTag(tag)) return TAG_COLOR[tag];
  return customTag(tag, tags)?.color ?? UNKNOWN_TAG_COLOR;
}

/** Sort order of tags: the built-ins in TablePlus's order, then custom tags by name, then none. */
export function compareTags(
  a: ConnectionTag | null,
  b: ConnectionTag | null,
  tags: readonly CustomTag[] = useCustomTagsStore.getState().tags,
): number {
  const rank = (t: ConnectionTag | null) =>
    t === null
      ? CONNECTION_TAGS.length + 2
      : isBuiltinTag(t)
        ? CONNECTION_TAGS.indexOf(t)
        : customTag(t, tags)
          ? CONNECTION_TAGS.length
          : CONNECTION_TAGS.length + 1;
  return (
    rank(a) - rank(b) ||
    (a !== null && b !== null && !isBuiltinTag(a) && !isBuiltinTag(b)
      ? a.localeCompare(b, undefined, { sensitivity: "base" })
      : 0)
  );
}

/**
 * Saves the custom tags. `renames` (old name → new name) are carried over to the profiles that
 * use them, so renaming a tag keeps its connections tagged.
 */
export function useSaveCustomTags() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({
      tags,
      renames,
      profiles,
    }: {
      tags: CustomTag[];
      renames: ReadonlyMap<string, string>;
      profiles: readonly ConnectionProfile[];
    }) => {
      const error = tagsError(tags);
      if (error) throw new Error(error);
      const clean = tags.map((t) => ({ name: t.name.trim(), color: t.color }));
      await ipc.settings.set(TAGS_SETTING, clean);
      useCustomTagsStore.setState({ tags: clean, loaded: true });
      const renamed = profiles.filter((p) => p.options.tag !== null && renames.has(p.options.tag));
      for (const p of renamed) {
        await ipc.profiles.save({
          profile: { ...p, options: { ...p.options, tag: renames.get(p.options.tag!)! } },
          password: null,
          sshPassword: null,
          sshPassphrase: null,
          clearPassword: false,
        });
      }
      if (renamed.length > 0) await qc.invalidateQueries({ queryKey: profileKeys.all });
      return clean;
    },
  });
}
