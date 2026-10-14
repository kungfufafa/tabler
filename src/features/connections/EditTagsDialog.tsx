import { Plus, Trash2 } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ACCENT_PRESETS } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { cssColor, HEX_COLOR } from "./colors";
import { CONNECTION_TAGS, TAG_NAME_MAX } from "./schema";
import { TAG_COLOR, TAG_LABEL, tagsError, useCustomTags, useSaveCustomTags } from "./tags";
import { useProfiles } from "./useConnections";

interface TagRow {
  key: string;
  /** The saved name (null for a tag added in this dialog), to carry renames over to profiles. */
  original: string | null;
  name: string;
  color: string;
}

/** A colour circle; `selected` gets the ring. */
function Swatch({
  color,
  selected,
  label,
  onClick,
}: {
  color: string;
  selected: boolean;
  label: string;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      aria-label={label}
      title={label}
      disabled={!onClick}
      onClick={onClick}
      className={cn(
        "ring-offset-background size-3.5 shrink-0 rounded-full ring-offset-1 outline-none",
        "focus-visible:ring-ring focus-visible:ring-2 disabled:cursor-default",
        selected ? "ring-foreground/60 ring-2" : onClick && "hover:scale-110",
      )}
      style={{ background: cssColor(color) }}
    />
  );
}

/**
 * Tag ▸ "Edit Tags…": the five built-in tags (fixed) and the custom tags (b494), each with a name
 * and a colour (an accent preset or any colour). Renaming a tag renames it in the connections
 * that use it; a deleted tag stays on its connections, in gray.
 */
export function EditTagsDialog({
  open,
  onOpenChange,
  current = null,
  onRenamed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The tag of the connection being edited (its uses are counted like the saved ones). */
  current?: string | null;
  onRenamed?: (from: string, to: string) => void;
}) {
  const uid = useId();
  const saved = useCustomTags();
  const profiles = useProfiles();
  const save = useSaveCustomTags();
  const [rows, setRows] = useState<TagRow[]>([]);

  useEffect(() => {
    if (!open) return;
    setRows(
      saved.map((t, i) => ({ key: `t${i}`, original: t.name, name: t.name, color: t.color })),
    );
    save.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const uses = useMemo(() => {
    const out = new Map<string, number>();
    for (const p of profiles.data ?? []) {
      const tag = p.options.tag?.toLowerCase();
      if (tag) out.set(tag, (out.get(tag) ?? 0) + 1);
    }
    return out;
  }, [profiles.data]);

  const error = tagsError(rows);
  const update = (key: string, patch: Partial<TagRow>) =>
    setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...patch } : r)));
  const add = () => {
    const taken = new Set(rows.map((r) => r.color));
    const color = ACCENT_PRESETS.find((c) => !taken.has(c)) ?? "gray";
    const key = `n${Date.now()}`;
    setRows((rs) => [...rs, { key, original: null, name: "", color }]);
    requestAnimationFrame(() => document.getElementById(`${uid}-${key}`)?.focus());
  };

  const submit = async () => {
    if (error) return;
    const renames = new Map<string, string>();
    for (const r of rows)
      if (r.original !== null && r.original !== r.name.trim())
        renames.set(r.original, r.name.trim());
    try {
      await save.mutateAsync({
        tags: rows.map((r) => ({ name: r.name.trim(), color: r.color })),
        renames,
        profiles: profiles.data ?? [],
      });
    } catch (err) {
      toast.error("Could not save the tags", {
        description: err instanceof Error ? err.message : String(err),
      });
      return;
    }
    for (const [from, to] of renames) onRenamed?.(from, to);
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col gap-3 sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Edit Tags</DialogTitle>
          <DialogDescription>
            Tags mark a connection&rsquo;s environment in the list and the status bar. Renaming a
            tag renames it on its connections.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex min-h-0 flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <ul aria-label="Tags" className="flex min-h-0 flex-col gap-1.5 overflow-y-auto">
            {CONNECTION_TAGS.map((t) => (
              <li key={t} className="flex h-8 items-center gap-2.5 px-1">
                <Swatch color={TAG_COLOR[t]} selected={false} label={TAG_COLOR[t]} />
                <span className="text-sm">{TAG_LABEL[t]}</span>
                <span className="text-muted-foreground ml-auto text-[11px]">Built-in</span>
              </li>
            ))}
            {rows.map((r) => {
              const count = r.original ? (uses.get(r.original.toLowerCase()) ?? 0) : 0;
              const hex = HEX_COLOR.test(r.color);
              return (
                <li key={r.key} className="flex items-center gap-2 px-1" data-testid="custom-tag">
                  <Input
                    id={`${uid}-${r.key}`}
                    value={r.name}
                    maxLength={TAG_NAME_MAX}
                    onChange={(e) => update(r.key, { name: e.target.value })}
                    placeholder="Tag name"
                    aria-label="Tag name"
                    className="h-7 min-w-0 flex-1 text-sm"
                  />
                  <div
                    role="radiogroup"
                    aria-label={`Colour of ${r.name || "the new tag"}`}
                    className="flex shrink-0 items-center gap-1"
                  >
                    {ACCENT_PRESETS.map((c) => (
                      <Swatch
                        key={c}
                        color={c}
                        label={c}
                        selected={r.color === c}
                        onClick={() => update(r.key, { color: c })}
                      />
                    ))}
                    <input
                      type="color"
                      aria-label="Other colour"
                      title="Other colour"
                      value={hex ? r.color : "#888888"}
                      onChange={(e) => update(r.key, { color: e.target.value })}
                      className={cn(
                        "size-4.5 shrink-0 cursor-pointer rounded border-0 bg-transparent p-0",
                        hex && "ring-foreground/60 ring-2",
                      )}
                    />
                  </div>
                  <span
                    className="text-muted-foreground w-14 shrink-0 text-right text-[11px] tabular-nums"
                    title={`${count} connection${count === 1 ? "" : "s"} use this tag`}
                  >
                    {count > 0 ? `${count} used` : ""}
                  </span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-7 shrink-0"
                    aria-label={`Delete ${r.name || "tag"}`}
                    title={count > 0 ? "Delete (its connections keep the tag, in gray)" : "Delete"}
                    onClick={() => setRows((rs) => rs.filter((x) => x.key !== r.key))}
                  >
                    <Trash2 />
                  </Button>
                </li>
              );
            })}
          </ul>
          <div className="flex items-center gap-2">
            <Button type="button" variant="outline" size="sm" onClick={add}>
              <Plus /> Add Tag
            </Button>
            {error && rows.length > 0 && (
              <p role="alert" className="text-error min-w-0 truncate text-[11px]">
                {error}
              </p>
            )}
            {current &&
              !rows.some((r) => r.name.trim() === current) &&
              !CONNECTION_TAGS.some((t) => t === current) && (
                <p className="text-muted-foreground min-w-0 truncate text-[11px]">
                  This connection&rsquo;s tag &ldquo;{current}&rdquo; is not defined here
                </p>
              )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={error !== null || save.isPending}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
