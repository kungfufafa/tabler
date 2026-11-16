import { Columns3 } from "lucide-react";
import { useLayoutEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ColumnMeta } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { BAR_HOVER, BarTooltip } from "./BarParts";

/** More columns than this and the menu offers a search field. */
const SEARCH_FROM = 12;

/**
 * The bottom bar's column visibility menu (⌥⌘F opens it from the keyboard, so its open state is
 * controlled by the table tab).
 */
export function ColumnsMenu({
  columns,
  hidden,
  open,
  onOpenChange,
  onToggleColumn,
  onShowAll,
}: {
  columns: ColumnMeta[];
  hidden: string[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onToggleColumn: (name: string, visible: boolean) => void;
  onShowAll: () => void;
}) {
  // The menu hands the focus back to where it was (usually the grid, when ⌥⌘F opened it) rather
  // than to its trigger. Recorded before the menu focuses itself.
  const returnFocus = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!open) return;
    const el = document.activeElement;
    returnFocus.current = el instanceof HTMLElement && el !== document.body ? el : null;
  }, [open]);
  const shown = columns.filter((c) => !hidden.includes(c.name)).length;
  const someHidden = shown < columns.length;
  // A wide table gets a search field instead of a long scroll; cleared as the menu closes.
  const [query, setQuery] = useState("");
  const openChange = (next: boolean) => {
    if (!next) setQuery("");
    onOpenChange(next);
  };
  const searchable = columns.length > SEARCH_FROM;
  const needle = query.trim().toLowerCase();
  const listed =
    searchable && needle ? columns.filter((c) => c.name.toLowerCase().includes(needle)) : columns;
  return (
    <DropdownMenu open={open} onOpenChange={openChange}>
      <BarTooltip label="Show / hide columns" action="grid.columns">
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            aria-label="Columns"
            className={cn(
              "text-muted-foreground hover:text-foreground transition-colors @max-sm:px-1",
              BAR_HOVER,
              someHidden && "text-brand hover:text-brand",
            )}
          >
            <Columns3 />
            <span className="@max-3xl:hidden">Columns</span>
            {someHidden && (
              <span className="tabular-nums">
                {shown}/{columns.length}
              </span>
            )}
          </Button>
        </DropdownMenuTrigger>
      </BarTooltip>
      <DropdownMenuContent
        side="top"
        align="end"
        className="max-h-80 w-56 overflow-auto"
        onCloseAutoFocus={(e) => {
          const el = returnFocus.current;
          returnFocus.current = null;
          if (!el?.isConnected) return;
          e.preventDefault();
          el.focus({ preventScroll: true });
        }}
      >
        <DropdownMenuLabel className="flex items-center justify-between">
          Columns
          <button
            type="button"
            className="text-brand focus-visible:ring-ring/50 rounded-sm text-xs font-normal outline-none hover:underline focus-visible:ring-2 disabled:opacity-50 disabled:hover:no-underline"
            onClick={onShowAll}
            disabled={!someHidden}
          >
            Show All
          </button>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {searchable && (
          <div className="px-1 pb-1">
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              // The menu's type-ahead would take the letters; ↓ and ⎋ still reach the menu.
              onKeyDown={(e) => {
                if (e.key !== "Escape" && e.key !== "ArrowDown" && e.key !== "Tab")
                  e.stopPropagation();
              }}
              placeholder="Search columns"
              aria-label="Search columns"
              spellCheck={false}
              autoComplete="off"
              className="h-6 text-xs md:text-xs"
            />
          </div>
        )}
        {listed.map((c) => {
          const visible = !hidden.includes(c.name);
          return (
            <DropdownMenuCheckboxItem
              key={c.name}
              checked={visible}
              // The grid keeps at least one column (the header menu's Hide Column does the same).
              disabled={visible && shown <= 1}
              onCheckedChange={(v) => onToggleColumn(c.name, v === true)}
              onSelect={(e) => e.preventDefault()}
              className="font-mono"
            >
              {c.name}
            </DropdownMenuCheckboxItem>
          );
        })}
        {listed.length === 0 && (
          <p className="text-muted-foreground px-2 py-1.5 text-xs">No matching columns</p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
