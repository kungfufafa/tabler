import {
  ChevronDown,
  CirclePlay,
  FileDown,
  FolderOpen,
  History,
  ListTree,
  Loader2,
  Lock,
  Play,
  Save,
  Square,
  TextAlignStart,
  type LucideIcon,
} from "lucide-react";
import type { MouseEvent, ReactNode } from "react";
import { ColorDot } from "@/components/ColorDot";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import type { ActionId } from "@/features/palette/actions";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useQueryTabField } from "@/stores/queryTabs";
import { baseName } from "./files";
import { ElapsedTimer } from "./ElapsedTimer";

export interface ToolbarHandlers {
  run: () => void;
  runCurrent: () => void;
  cancel: () => void;
  explain: () => void;
  analyze: () => void;
  format: () => void;
  history: () => void;
  open: () => void;
  save: () => void;
  saveAs: () => void;
  /** Opens the export dialog on the selection / statement under the cursor. */
  exportResults: () => void;
}

function Tip({
  label,
  action,
  children,
}: {
  label: string;
  action?: ActionId;
  children: ReactNode;
}) {
  const combo = action ? shortcutFor(action) : undefined;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

function IconButton({
  icon: Icon,
  label,
  action,
  onClick,
  disabled,
  className,
}: {
  icon: LucideIcon;
  label: string;
  action?: ActionId;
  onClick: (e: MouseEvent<HTMLButtonElement>) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <Tip label={label} action={action}>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={label}
        onClick={onClick}
        disabled={disabled}
        className={className}
      >
        <Icon />
      </Button>
    </Tip>
  );
}

const Divider = () => <div className="bg-border mx-1 h-4 w-px shrink-0" aria-hidden />;

/** Database label: the MySQL / PG database, or the SQLite file name. */
function databaseLabel(driver: string | undefined, db: string | null | undefined): string | null {
  if (!db) return null;
  return driver === "sqlite" ? baseName(db) : db;
}

/**
 * The toolbar is a size container so it degrades instead of overflowing when the main panel is
 * narrow (minimum window, sidebar and inspector open): below 640px the Explain caption, the running
 * timer (the results header shows it too) and the database name go; below 440px the Run caption;
 * below 400px the connection name (the colour dot and its tooltip stay).
 */
export function QueryToolbar({
  tabId,
  connectionId,
  handlers,
}: {
  tabId: string;
  connectionId: string;
  handlers: ToolbarHandlers;
}) {
  const run = useQueryTabField(tabId, "run");
  const plan = useQueryTabField(tabId, "plan");
  const info = useConnectionsStore((s) => s.byId[connectionId]?.info);
  const running = run?.status === "running";
  const busy = running || plan?.status === "running";
  const since = running ? run.startedAt : plan?.status === "running" ? plan.startedAt : null;
  const db = databaseLabel(info?.driver, info?.currentDatabase);

  return (
    <div
      role="toolbar"
      aria-label="Query"
      className="border-border bg-surface-2 @container flex h-9 shrink-0 items-center gap-0.5 border-b px-2"
    >
      <Tip label="Run selection, or everything" action="query.run">
        <Button
          size="sm"
          aria-label="Run"
          className="h-7 gap-1.5 px-2.5 text-xs @max-[440px]:px-2"
          onClick={handlers.run}
          disabled={busy}
        >
          <Play className="fill-current" />
          <span className="@max-[440px]:hidden">Run</span>
        </Button>
      </Tip>
      <IconButton
        icon={CirclePlay}
        label="Run statement under cursor"
        action="query.run-current"
        onClick={handlers.runCurrent}
        disabled={busy}
      />
      <IconButton
        icon={Square}
        label={run?.cancelling ? "Stopping…" : "Stop"}
        action="query.cancel"
        onClick={handlers.cancel}
        disabled={!busy || run?.cancelling}
        className={cn(busy && "text-error hover:text-error")}
      />
      <Divider />
      <div className="flex items-center">
        <Tip label="Explain (⇧-click: Explain Analyze)" action="query.explain">
          <Button
            variant="ghost"
            size="sm"
            aria-label="Explain"
            className="h-7 gap-1 rounded-r-none px-2 text-xs"
            onClick={(e) => (e.shiftKey ? handlers.analyze() : handlers.explain())}
            disabled={busy}
          >
            <ListTree />
            <span className="@max-[640px]:hidden">Explain</span>
          </Button>
        </Tip>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="w-5 rounded-l-none"
              aria-label="Explain options"
              disabled={busy}
            >
              <ChevronDown className="size-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-52">
            <DropdownMenuItem onSelect={handlers.explain}>
              Explain
              <DropdownMenuShortcut>
                {formatCombo(shortcutFor("query.explain") ?? "")}
              </DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={handlers.analyze}>
              Explain Analyze
              <DropdownMenuShortcut>⇧ click</DropdownMenuShortcut>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <IconButton
        icon={TextAlignStart}
        label="Format SQL"
        action="query.format"
        onClick={handlers.format}
      />
      <Divider />
      <IconButton
        icon={History}
        label="History"
        action="query.history"
        onClick={handlers.history}
      />
      <IconButton
        icon={FolderOpen}
        label="Open .sql file"
        action="query.open"
        onClick={handlers.open}
      />
      <IconButton
        icon={Save}
        label="Save (⇧-click: Save As…)"
        action="query.save"
        onClick={(e) => (e.shiftKey ? handlers.saveAs() : handlers.save())}
      />
      <Divider />
      <IconButton
        icon={FileDown}
        label="Export results…"
        action="export.open"
        onClick={handlers.exportResults}
      />

      <span className="flex-1" />

      {since !== null && (
        <span className="text-muted-foreground mr-2 flex shrink-0 items-center gap-1.5 text-[11px] whitespace-nowrap @max-[640px]:hidden">
          <Loader2 className="text-brand size-3 animate-spin" />
          <ElapsedTimer since={since} />
        </span>
      )}
      {info ? (
        <span
          className="text-muted-foreground ml-1 flex min-w-0 items-center gap-1.5 text-[11px] whitespace-nowrap"
          title={`${info.name}${db ? ` · ${info.currentDatabase}` : ""} · ${info.serverVersion}${info.readOnly ? " · read-only" : ""}`}
        >
          <ColorDot color={info.color} size={7} />
          <span className="text-foreground/80 min-w-0 truncate font-medium @max-[400px]:hidden">
            {info.name}
          </span>
          {db && (
            <>
              <span className="text-border-strong @max-[640px]:hidden">/</span>
              <span className="min-w-0 truncate font-mono @max-[640px]:hidden">{db}</span>
            </>
          )}
          {info.readOnly && (
            <span className="border-warning/40 text-warning flex shrink-0 items-center gap-0.5 rounded border px-1 text-[10px]">
              <Lock className="size-2.5" />
              <span className="@max-[400px]:hidden">read-only</span>
            </span>
          )}
        </span>
      ) : (
        <span className="text-muted-foreground shrink-0 text-[11px] whitespace-nowrap">
          Not connected
        </span>
      )}
    </div>
  );
}
