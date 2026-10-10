import { Columns3, Filter, RefreshCw, TableProperties } from "lucide-react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ColumnMeta, TableRef } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export function TableToolbar({
  table,
  columns,
  hidden,
  filtersVisible,
  activeFilters,
  loading,
  onToggleFilters,
  onRefresh,
  onToggleColumn,
  onShowAllColumns,
  onOpenStructure,
  sqlPreview,
}: {
  table: TableRef;
  columns: ColumnMeta[];
  hidden: string[];
  filtersVisible: boolean;
  activeFilters: number;
  loading: boolean;
  onToggleFilters: () => void;
  onRefresh: () => void;
  onToggleColumn: (name: string, visible: boolean) => void;
  onShowAllColumns: () => void;
  onOpenStructure: () => void;
  sqlPreview: string | null;
}) {
  return (
    <div className="border-border bg-surface-2 flex h-9 shrink-0 items-center gap-1 border-b px-2">
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="min-w-0 truncate font-mono text-xs">
            {table.schema && <span className="text-muted-foreground">{table.schema}.</span>}
            <span className="font-semibold">{table.name}</span>
          </span>
        </TooltipTrigger>
        {sqlPreview && (
          <TooltipContent
            side="bottom"
            align="start"
            className="max-w-lg font-mono text-[11px] whitespace-pre-wrap"
          >
            {sqlPreview}
          </TooltipContent>
        )}
      </Tooltip>
      <span className="flex-1" />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant={filtersVisible || activeFilters > 0 ? "secondary" : "ghost"}
            size="sm"
            onClick={onToggleFilters}
            className={cn(activeFilters > 0 && "text-brand")}
          >
            <Filter />
            Filter{activeFilters > 0 ? ` (${activeFilters})` : ""}
          </Button>
        </TooltipTrigger>
        <TooltipContent className="flex items-center gap-1.5">
          Toggle filters{" "}
          <Kbd combo="Mod+F" className="border-border/40 bg-transparent text-inherit" />
        </TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="sm" aria-label="Columns">
            <Columns3 />
            {hidden.length > 0 && (
              <span className="text-muted-foreground">
                {columns.length - hidden.length}/{columns.length}
              </span>
            )}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="max-h-80 w-56 overflow-auto">
          <DropdownMenuLabel className="flex items-center justify-between">
            Columns
            <button
              type="button"
              className="text-brand text-[11px] font-normal hover:underline"
              onClick={onShowAllColumns}
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
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-sm" onClick={onOpenStructure} aria-label="Structure">
            <TableProperties />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Structure</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={onRefresh}
            aria-label="Refresh"
            disabled={loading}
          >
            <RefreshCw className={cn(loading && "animate-spin")} />
          </Button>
        </TooltipTrigger>
        <TooltipContent className="flex items-center gap-1.5">
          Refresh <Kbd combo="Mod+R" className="border-border/40 bg-transparent text-inherit" />
        </TooltipContent>
      </Tooltip>
    </div>
  );
}
