import { Ban } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
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
import { cn } from "@/lib/utils";
import {
  childNames,
  childPath,
  GROUP_ICON_IDS,
  GROUP_ICONS,
  groupNameError,
  iconOf,
  leafName,
  parentPath,
  useGroupActions,
  useStoredGroups,
  type GroupIconId,
} from "./groups";
import type { GroupDialogRequest } from "./ioStore";

const CHOICES: ReadonlyArray<GroupIconId | null> = [null, ...GROUP_ICON_IDS];
const COLUMNS = 5;

/**
 * The group icon choices: "None" and the curated icons, as a radio grid (arrow keys move the
 * choice, like the other pickers of the app).
 */
export function GroupIconPicker({
  value,
  onChange,
}: {
  value: GroupIconId | null;
  onChange: (icon: GroupIconId | null) => void;
}) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const onKeyDown = (e: React.KeyboardEvent, i: number) => {
    const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLUMNS, ArrowUp: -COLUMNS }[e.key];
    if (step === undefined) return;
    e.preventDefault();
    const next = Math.max(0, Math.min(CHOICES.length - 1, i + step));
    onChange(CHOICES[next] ?? null);
    refs.current[next]?.focus();
  };
  return (
    <div role="radiogroup" aria-label="Group icon" className="grid grid-cols-5 gap-1.5">
      {CHOICES.map((id, i) => {
        const Icon = id ? GROUP_ICONS[id].icon : Ban;
        const label = id ? GROUP_ICONS[id].label : "None";
        const checked = value === id;
        return (
          <button
            key={id ?? "none"}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            aria-label={label}
            title={label}
            tabIndex={checked ? 0 : -1}
            onClick={() => onChange(id)}
            onKeyDown={(e) => onKeyDown(e, i)}
            className={cn(
              "border-border text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:ring-ring/50 flex h-9 items-center justify-center rounded-md border outline-none focus-visible:ring-2",
              checked &&
                "border-brand bg-brand-soft text-brand hover:bg-brand-soft hover:text-brand",
            )}
          >
            <Icon className={cn("size-4", !id && "opacity-60")} aria-hidden />
          </button>
        );
      })}
    </div>
  );
}

/**
 * "New Group…" / "New Subgroup…" / "Rename Group…" (a name, checked against the other groups of
 * the same parent without case, and an icon) and "Change Icon…" (the icon only). `onSubmit`
 * receives the group's full new path; the dialog saves the icon itself.
 */
export function GroupNameDialog({
  request,
  groups,
  onSubmit,
  onClose,
}: {
  request: GroupDialogRequest | null;
  /** Every existing group path. */
  groups: readonly string[];
  /** Creates (`new`) or renames (`rename`) the group; `name` is its full path. */
  onSubmit: (request: GroupDialogRequest, name: string) => Promise<boolean>;
  onClose: () => void;
}) {
  const [last, setLast] = useState(request);
  if (request && request !== last) setLast(request);
  const shown = request ?? last;
  const id = useId();
  const stored = useStoredGroups();
  const actions = useGroupActions();
  const [name, setName] = useState("");
  const [icon, setIcon] = useState<GroupIconId | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const existing = shown && shown.kind !== "new" ? shown.group : null;
  const parent = shown?.kind === "new" ? shown.parent : existing ? parentPath(existing) : "";
  const savedIcon = existing ? iconOf(stored.data ?? [], existing) : null;

  useEffect(() => {
    if (!request) return;
    setName(request.kind === "new" ? "" : leafName(request.group));
    setIcon(request.kind === "new" ? null : iconOf(stored.data ?? [], request.group));
    setError(null);
    setBusy(false);
    // Only when a new request arrives: the saved icon is the starting value.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!request) return;
    let path = existing ?? "";
    if (request.kind !== "icon") {
      const current = existing ? leafName(existing) : null;
      const problem = groupNameError(name, childNames(groups, parent), current, parent);
      if (problem) {
        setError(problem);
        return;
      }
      path = childPath(parent, name);
    }
    setBusy(true);
    let ok = path === existing || (await onSubmit(request, path));
    if (ok && icon !== savedIcon) ok = await actions.setIcon(path, icon);
    setBusy(false);
    if (ok) onClose();
  };

  const title =
    shown?.kind === "rename"
      ? "Rename Group"
      : shown?.kind === "icon"
        ? "Change Icon"
        : parent
          ? "New Subgroup"
          : "New Group";
  const description =
    shown?.kind === "rename"
      ? `The connections and subgroups of “${shown.group}” move with it.`
      : shown?.kind === "icon"
        ? `Shown on the header of “${shown.group}”.`
        : parent
          ? `Inside “${parent}”.`
          : "Groups sort connections on the welcome screen. Drag connections onto a group to move them.";

  return (
    <Dialog open={request !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </DialogHeader>
          {shown?.kind !== "icon" && (
            <div className="flex flex-col gap-1">
              <Input
                id={`${id}-name`}
                aria-label="Group name"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setError(null);
                }}
                aria-invalid={error ? true : undefined}
                placeholder="Group name"
                autoComplete="off"
                autoFocus
              />
              {error && (
                <p role="alert" className="text-error text-[11px]">
                  {error}
                </p>
              )}
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <span className="text-muted-foreground text-xs">Icon</span>
            <GroupIconPicker value={icon} onChange={setIcon} />
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              {shown?.kind === "rename" ? "Rename" : shown?.kind === "icon" ? "Save" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
