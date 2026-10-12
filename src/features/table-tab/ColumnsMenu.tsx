import { Columns3 } from "lucide-react";
import { useLayoutEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
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
import { BarTooltip } from "./BarParts";

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
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange}>
      <BarTooltip label="Show / hide columns" action="grid.columns">
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            aria-label="Columns"
            className={cn(
              "text-muted-foreground hover:text-foreground h-6 px-1.5",
              someHidden && "text-brand hover:text-brand",
            )}
          >
            <Columns3 />
            <span className="@max-2xl:hidden">Columns</span>
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
            className="text-brand text-[11px] font-normal hover:underline disabled:opacity-50 disabled:hover:no-underline"
            onClick={onShowAll}
            disabled={!someHidden}
          >
            Show all
          </button>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {columns.map((c) => (
          <DropdownMenuCheckboxItem
            key={c.name}
            checked={!hidden.includes(c.name)}
            onCheckedChange={(v) => onToggleColumn(c.name, v === true)}
            onSelect={(e) => e.preventDefault()}
            className="font-mono text-xs"
          >
            {c.name}
          </DropdownMenuCheckboxItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
