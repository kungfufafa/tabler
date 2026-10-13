import { ChevronsUpDown, Folder } from "lucide-react";
import { useId, useMemo, useRef, useState } from "react";
import { Input } from "@/components/ui/input";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import { leafName, normalizePath, parentPath, pathDepth } from "./groups";

/** The suggestions for `query`: every path while nothing was typed, else those containing it. */
function groupSuggestions(paths: readonly string[], query: string | null): string[] {
  const q = normalizePath(query ?? "").toLowerCase();
  if (q === "") return [...paths];
  return paths.filter((p) => p.toLowerCase().includes(q));
}

/**
 * The connection form's Group field: free text (a nested path like `Clients / Acme`) with the
 * existing group paths suggested in a list under it. ↓ / ↑ move through the list, ↩ picks, Esc
 * closes it (the dialog stays open).
 */
export function GroupCombobox({
  id,
  value,
  onChange,
  onBlur,
  groups,
  placeholder,
  invalid,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  /** Existing group paths, in tree order. */
  groups: readonly string[];
  placeholder?: string;
  invalid?: boolean;
}) {
  const listId = useId();
  const input = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  /** What was typed since the list opened (null: nothing, so every group is listed). */
  const [query, setQuery] = useState<string | null>(null);
  const [active, setActive] = useState(-1);
  const items = useMemo(() => groupSuggestions(groups, query), [groups, query]);
  const shown = open && items.length > 0;
  const activeItem = shown && active >= 0 ? items[active] : undefined;

  const show = () => {
    setQuery(null);
    setActive(-1);
    setOpen(true);
  };
  const pick = (path: string) => {
    onChange(path);
    setOpen(false);
    setActive(-1);
  };
  const optionId = (i: number) => `${listId}-${i}`;

  return (
    <Popover open={shown} onOpenChange={(o) => !o && setOpen(false)}>
      <PopoverAnchor asChild>
        <div className="relative">
          <Input
            ref={input}
            id={id}
            role="combobox"
            aria-expanded={shown}
            aria-controls={shown ? listId : undefined}
            aria-autocomplete="list"
            aria-activedescendant={activeItem !== undefined ? optionId(active) : undefined}
            aria-invalid={invalid ? true : undefined}
            autoComplete="off"
            spellCheck={false}
            placeholder={placeholder}
            value={value}
            className="pr-8"
            onChange={(e) => {
              onChange(e.target.value);
              setQuery(e.target.value);
              setActive(-1);
              setOpen(true);
            }}
            onFocus={show}
            onClick={() => !open && show()}
            onBlur={() => {
              setOpen(false);
              onBlur?.();
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                if (!shown) {
                  show();
                  return;
                }
                const down = e.key === "ArrowDown";
                const n = items.length;
                setActive((i) => (down ? (i + 1) % n : i <= 0 ? n - 1 : i - 1));
                return;
              }
              if (e.key === "Enter" && activeItem !== undefined) {
                // Picks the suggestion instead of submitting the form.
                e.preventDefault();
                pick(activeItem);
                return;
              }
              if (e.key === "Escape" && shown) {
                // Closes the list only, not the dialog.
                e.preventDefault();
                e.stopPropagation();
                setOpen(false);
                return;
              }
              if (e.key === "Tab") setOpen(false);
            }}
          />
          <button
            type="button"
            tabIndex={-1}
            aria-label="Show groups"
            className="text-muted-foreground hover:text-foreground absolute inset-y-0 right-0 flex w-8 items-center justify-center"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              input.current?.focus();
              if (open) setOpen(false);
              else show();
            }}
          >
            <ChevronsUpDown className="size-3.5" />
          </button>
        </div>
      </PopoverAnchor>
      <PopoverContent
        align="start"
        sideOffset={2}
        className="w-(--radix-popover-trigger-width) gap-0 p-1"
        // Typing stays in the field.
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => e.preventDefault()}
        onInteractOutside={(e) => {
          if (e.target instanceof Node && input.current?.parentElement?.contains(e.target))
            e.preventDefault();
        }}
      >
        <div
          id={listId}
          role="listbox"
          aria-label="Groups"
          className="flex max-h-56 flex-col overflow-y-auto"
        >
          {items.map((path, i) => {
            const parent = parentPath(path);
            return (
              <div
                key={path}
                id={optionId(i)}
                role="option"
                aria-selected={i === active}
                title={path}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(path)}
                className={cn(
                  "flex min-w-0 cursor-default items-center gap-1.5 rounded-md px-2 py-1 text-xs",
                  i === active && "bg-accent text-accent-foreground",
                  path === value && "font-medium",
                )}
                // The whole tree is indented; filtered results show their full path instead.
                style={
                  query === null
                    ? { paddingLeft: `${0.5 + Math.min(pathDepth(path), 6) * 0.75}rem` }
                    : undefined
                }
              >
                <Folder className="text-muted-foreground size-3.5 shrink-0" />
                <span className="min-w-0 truncate">
                  {query !== null && parent ? (
                    <span className="text-muted-foreground">{parent} / </span>
                  ) : null}
                  {leafName(path)}
                </span>
              </div>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}
