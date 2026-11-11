import {
  BookmarkPlus,
  ChevronDown,
  Ellipsis,
  FileDown,
  FolderOpen,
  History,
  ListTree,
  PanelBottomClose,
  PanelBottomOpen,
  Play,
  Save,
  Square,
  TextAlignStart,
  type LucideIcon,
} from "lucide-react";
import type { MouseEvent, ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ActionId } from "@/features/palette/actions";
import { formatCount, formatDuration } from "@/lib/format/time";
import { formatCombo, isMac } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { appliesPartially, has, useCaps } from "@/lib/caps";
import { cn } from "@/lib/utils";
import { updateQueryTab, useQueryTabField } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { useResultRowCount } from "@/stores/resultData";
import { ROW_LIMITS } from "./controller";
import { useCursorLabel } from "./cursor";
import { EditorSettingsMenu } from "./EditorSettingsMenu";
import { ElapsedTimer } from "./ElapsedTimer";
import { runSummaryText } from "./labels";
import { resolvePane } from "./panes";
import { RUN_BUTTON_ACTION, useRunButtonStore, type RunButtonMode } from "./runButton";
import { useActiveEditorKey } from "./split";
import { summarize } from "./runModel";

export interface EditorBarHandlers {
  /** Runs the selection, or everything (⇧⌘↩). */
  run: () => void;
  /** Runs the selection, or the statement under the cursor (⌘↩). */
  runCurrent: () => void;
  cancel: () => void;
  explain: () => void;
  analyze: () => void;
  /** Beautify SQL (⌘I). */
  format: () => void;
  /** Uglify SQL (⇧⌘I). */
  minify: () => void;
  /** Collapses / expands the result pane (⌃`). */
  toggleResults: () => void;
  history: () => void;
  open: () => void;
  save: () => void;
  saveAs: () => void;
  /** "Save Selected SQL Statements As…" (⇧⌘S). */
  saveSelection: () => void;
  /** "Add to Queries…" (⌥⌘S): the selection / current statement as a saved query. */
  addToQueries: () => void;
  /** Opens the export dialog on the shown result, or the selection / statement under the cursor. */
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
        className={cn(
          "text-muted-foreground hover:text-foreground rounded-md transition-colors",
          className,
        )}
      >
        <Icon />
      </Button>
    </Tip>
  );
}

const Divider = ({ className }: { className?: string }) => (
  <div className={cn("bg-border h-4 w-px shrink-0", className)} aria-hidden />
);

/** An action's primary shortcut, right-aligned in a menu item. */
function MenuShortcut({ action }: { action: ActionId }) {
  const combo = shortcutFor(action);
  return combo ? <DropdownMenuShortcut>{formatCombo(combo)}</DropdownMenuShortcut> : null;
}

/** An action's shortcut inside a bar button (hidden when the bar is narrow). */
function ButtonKbd({ action, className }: { action: ActionId; className?: string }) {
  const combo = shortcutFor(action);
  if (!combo) return null;
  return (
    <span aria-hidden className={className}>
      <Kbd combo={combo} className="h-4 border-current/30 bg-transparent text-inherit" />
    </span>
  );
}

/** A ⇧-click, spelled the platform's way. */
const SHIFT_CLICK = isMac ? "⇧-click" : "Shift+Click";

/** Styles shared by the bar's split buttons (a labelled button + its ⌄ menu). */
const splitMain = "h-7 gap-1 rounded-l-md rounded-r-none px-1 text-[12px] transition-colors";
const splitMenu = "h-7 w-5 rounded-l-none rounded-r-md px-0 transition-colors";
const runTone = "bg-foreground text-background hover:bg-foreground/90";

/**
 * "Line 4, Column 13, Location 40" (+ the selection size) of the pane used last when split. It
 * truncates before the buttons on the right overflow: their shortcut hints are wider off macOS
 * ("Ctrl+Enter" for "⌘↩"), which the bar's breakpoints do not account for.
 */
function CursorPosition({ tabId }: { tabId: string }) {
  const label = useCursorLabel(useActiveEditorKey(tabId));
  return (
    <span
      data-testid="cursor-position"
      title={label}
      className="text-muted-foreground min-w-0 truncate px-1 text-[12px] whitespace-nowrap tabular-nums @max-[760px]:max-w-28 @max-[560px]:hidden"
    >
      {label}
    </span>
  );
}

/**
 * What the tab's last run did ("3 statements · 200 rows · 45 ms", errors in red), the running
 * statement with a timer, or the plan's outcome while the Plan pane shows.
 */
function RunSummary({ tabId }: { tabId: string }) {
  const run = useQueryTabField(tabId, "run");
  const plan = useQueryTabField(tabId, "plan");
  const pane = useQueryTabField(tabId, "pane");
  const results = run?.results;
  const active = resolvePane(pane, results ?? [], plan);
  const shown = results?.find((r) => r.id === active) ?? null;
  const rows = useResultRowCount(shown?.id ?? null);
  const box = "flex min-w-0 items-center gap-1 truncate text-[12px] whitespace-nowrap";

  if (run?.status === "running") {
    const n = run.statements.length;
    return (
      <span className="sr-only">
        <span className="truncate">
          {run.cancelling ? "Stopping…" : n > 1 ? `Running statement ${n}…` : "Running…"}
        </span>
        <ElapsedTimer since={run.startedAt} />
      </span>
    );
  }
  if (plan?.status === "running") {
    return (
      <span className="sr-only">
        <span className="truncate">Explaining…</span>
        <ElapsedTimer since={plan.startedAt} />
      </span>
    );
  }
  if (active === "plan" && plan) {
    const title = plan.mode === "analyze" ? "Explain analyze" : "Explain";
    if (plan.status === "error")
      return <span className={cn(box, "text-error")}>{title} failed</span>;
    if (plan.status === "cancelled")
      return <span className={cn(box, "text-warning")}>{title} cancelled</span>;
    return (
      <span className={cn(box, "text-muted-foreground tabular-nums")}>
        {title}
        {plan.result ? ` · ${formatDuration(plan.result.durationMs)}` : ""}
      </span>
    );
  }
  if (!run) return null;
  const s = summarize(run);
  return (
    <span className={box} title={run.sql.length > 500 ? `${run.sql.slice(0, 500)}…` : run.sql}>
      <span
        className={cn(
          "truncate tabular-nums",
          s.errors > 0 ? "text-error" : s.cancelled ? "text-warning" : "text-muted-foreground",
        )}
      >
        {runSummaryText(s, shown ? rows : null)}
      </span>
      {shown?.truncated && (
        <span
          className="text-warning shrink-0"
          title={`Only the first ${formatCount(rows)} rows were read. Raise the row limit to read more.`}
        >
          limit reached
        </span>
      )}
    </span>
  );
}

const LIMIT_VALUES = ROW_LIMITS.map((l) => (l === null ? "none" : String(l)));

function limitLabel(limit: number | null): string {
  return limit === null ? "No limit" : formatCount(limit);
}

/** Rows read per statement: "No limit" (the default), 100, 300, 500, 1,000 or 10,000 (per tab). */
function RowLimitSelect({ tabId }: { tabId: string }) {
  const limit = useQueryTabField(tabId, "rowLimit");
  const value = limit === null ? "none" : String(limit);
  return (
    <Select
      value={value}
      onValueChange={(v) => updateQueryTab(tabId, { rowLimit: v === "none" ? null : Number(v) })}
    >
      <Tip label="Rows read per statement">
        <SelectTrigger
          size="sm"
          aria-label="Row limit"
          className={cn(
            "text-muted-foreground hover:text-foreground hover:bg-muted dark:hover:bg-muted/50 gap-1 rounded-md border-transparent bg-transparent px-2 text-[12px] tabular-nums shadow-none data-[size=sm]:h-7 dark:bg-transparent [&_svg]:size-3.5",
            "data-[state=open]:bg-muted data-[state=open]:text-foreground",
          )}
        >
          <SelectValue>{limit === null ? "No limit" : `Limit ${limitLabel(limit)}`}</SelectValue>
        </SelectTrigger>
      </Tip>
      <SelectContent align="end" className="min-w-28">
        {ROW_LIMITS.map((l, i) => (
          <SelectItem key={LIMIT_VALUES[i]} value={LIMIT_VALUES[i]!} className="text-xs">
            {limitLabel(l)}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

const RUN_LABELS: Record<RunButtonMode, { caption: string; tip: string }> = {
  current: { caption: "Run Current", tip: "Run the current statement, or the selection" },
  all: { caption: "Run All", tip: "Run all statements, or the selection" },
};

/**
 * Run as a split button: the button runs the current statement or everything, whichever the menu
 * says it should (TablePlus lets you pick); the menu also runs either one. While a statement or an
 * explain runs, Cancel takes its place.
 */
function RunButton({
  handlers,
  busy,
  cancelling,
}: {
  handlers: EditorBarHandlers;
  busy: boolean;
  cancelling: boolean;
}) {
  const mode = useRunButtonStore((s) => s.mode);
  const setMode = useRunButtonStore((s) => s.setMode);
  if (busy) {
    return (
      <Tip label={cancelling ? "Stopping…" : "Cancel the running query"} action="query.cancel">
        <Button
          size="xs"
          variant="destructive"
          aria-label={cancelling ? "Stopping…" : "Cancel"}
          // As wide as Run, so the buttons beside it stay put when a run starts.
          className="h-7 min-w-[7.5rem] gap-2 rounded-md px-2 text-[12px] transition-colors @max-[760px]:min-w-0"
          onClick={handlers.cancel}
          disabled={cancelling}
        >
          <Square className="size-3 fill-current" />
          <span className="@max-[440px]:hidden">{cancelling ? "Stopping…" : "Cancel"}</span>
          <ButtonKbd action="query.cancel" className="@max-[760px]:hidden" />
        </Button>
      </Tip>
    );
  }
  const label = RUN_LABELS[mode];
  return (
    <div className="flex shrink-0 items-center">
      <Tip label={label.tip} action={RUN_BUTTON_ACTION[mode]}>
        <Button
          size="xs"
          aria-label={label.caption}
          className={cn(splitMain, runTone, "gap-2 px-2")}
          onClick={mode === "all" ? handlers.run : handlers.runCurrent}
        >
          <Play className="size-3.5 fill-current" />
          <span className="@max-[440px]:hidden">{label.caption}</span>
          <ButtonKbd action={RUN_BUTTON_ACTION[mode]} className="@max-[760px]:hidden" />
        </Button>
      </Tip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="xs"
            aria-label="Run options"
            className={cn(splitMenu, runTone, "border-l-background/20")}
          >
            <ChevronDown className="size-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-60">
          <DropdownMenuItem onSelect={handlers.runCurrent}>
            Run Current
            <MenuShortcut action="query.run-current" />
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.run}>
            Run All
            <MenuShortcut action="query.run" />
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={handlers.addToQueries}>
            Add to Queries…
            <MenuShortcut action="query.add-to-queries" />
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.saveAs}>Save SQL As…</DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.saveSelection}>
            Save Selected SQL Statements As…
            <MenuShortcut action="query.save-selection" />
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-muted-foreground text-[11px] font-normal">
            The Run button runs
          </DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={mode}
            onValueChange={(v) => setMode(v === "all" ? "all" : "current")}
          >
            <DropdownMenuRadioItem value="current">The current statement</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="all">All statements</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function BeautifyButton({ handlers }: { handlers: EditorBarHandlers }) {
  return (
    <div className="flex shrink-0 items-center">
      <Tip label="Beautify the selection, or the statement under the cursor" action="query.format">
        <Button
          variant="ghost"
          size="xs"
          aria-label="Beautify"
          className={splitMain}
          onClick={handlers.format}
        >
          <TextAlignStart className="size-4" />
          <span className="@max-[760px]:hidden">Beautify</span>
        </Button>
      </Tip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="xs" aria-label="Format options" className={splitMenu}>
            <ChevronDown className="size-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuItem onSelect={handlers.format} className="items-start">
            <span className="flex min-w-0 flex-col">
              Beautify SQL
              <span className="text-muted-foreground text-[11px] leading-4">
                The selection, or the statement under the cursor
              </span>
            </span>
            <MenuShortcut action="query.format" />
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.minify} className="items-start">
            <span className="flex min-w-0 flex-col">
              Uglify SQL
              <span className="text-muted-foreground text-[11px] leading-4">
                On one line, without comments
              </span>
            </span>
            <MenuShortcut action="query.minify" />
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function ExplainButton({
  handlers,
  busy,
  analyze = true,
  rollsBack = true,
}: {
  handlers: EditorBarHandlers;
  busy: boolean;
  /** Explain Analyze is offered. */
  analyze?: boolean;
  /** An analyzed statement runs in a transaction that is rolled back. */
  rollsBack?: boolean;
}) {
  const analyzeNote = rollsBack ? "Runs in a rolled-back transaction" : "Runs the statement";
  return (
    <div className="flex shrink-0 items-center">
      <Tip
        label={
          analyze
            ? `Explain (${SHIFT_CLICK}: Explain Analyze, which ${
                rollsBack ? "runs in a rolled-back transaction" : "runs the statement"
              })`
            : "Explain"
        }
        action="query.explain"
      >
        <Button
          variant="ghost"
          size="xs"
          aria-label="Explain"
          className={splitMain}
          onClick={(e) => (e.shiftKey && analyze ? handlers.analyze() : handlers.explain())}
          disabled={busy}
        >
          <ListTree className="size-4" />
          <span className="@max-[760px]:hidden">Explain</span>
        </Button>
      </Tip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            aria-label="Explain options"
            className={splitMenu}
            disabled={busy}
          >
            <ChevronDown className="size-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuItem onSelect={handlers.explain}>
            Explain
            <MenuShortcut action="query.explain" />
          </DropdownMenuItem>
          {analyze && (
            <DropdownMenuItem onSelect={handlers.analyze} className="items-start">
              <span className="flex min-w-0 flex-col">
                Explain Analyze
                <span className="text-muted-foreground text-[11px] leading-4">{analyzeNote}</span>
              </span>
              <DropdownMenuShortcut>{SHIFT_CLICK}</DropdownMenuShortcut>
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/** History, open / save and export: icon buttons, or one "⋯" menu when the bar is narrow. */
function FileActions({ handlers }: { handlers: EditorBarHandlers }) {
  return (
    <>
      <div className="hidden shrink-0 items-center @min-[840px]:flex">
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
          label={`Save (${SHIFT_CLICK}: Save As…)`}
          action="query.save"
          onClick={(e) => (e.shiftKey ? handlers.saveAs() : handlers.save())}
        />
        <IconButton
          icon={FileDown}
          label="Export results…"
          action="export.open"
          onClick={handlers.exportResults}
        />
      </div>
      <DropdownMenu>
        <Tip label="More query actions">
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="More query actions"
              className="text-muted-foreground hover:text-foreground rounded-md transition-colors @min-[840px]:hidden"
            >
              <Ellipsis />
            </Button>
          </DropdownMenuTrigger>
        </Tip>
        <DropdownMenuContent align="start" className="w-52">
          <DropdownMenuItem onSelect={handlers.history}>
            <History />
            History
            <MenuShortcut action="query.history" />
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={handlers.open}>
            <FolderOpen />
            Open .sql File…
            <MenuShortcut action="query.open" />
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.save}>
            <Save />
            Save
            <MenuShortcut action="query.save" />
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.saveAs}>
            <span className="size-4" aria-hidden />
            Save As…
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={handlers.addToQueries}>
            <BookmarkPlus />
            Add to Queries…
            <MenuShortcut action="query.add-to-queries" />
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={handlers.exportResults}>
            <FileDown />
            Export Results…
            <MenuShortcut action="export.open" />
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}

/**
 * The bar under the SQL editor, as in TablePlus: the ⚙ settings, the cursor position, the file
 * actions and the outcome of the last run on the left; the row limit, Beautify, Explain and Run on
 * the right. It is a size container so it degrades instead of overflowing when the main panel is
 * narrow. File actions fold into an overflow menu, status and secondary controls get less space,
 * and the groups wrap on the narrowest split workspaces.
 */
export function EditorBar({ tabId, handlers }: { tabId: string; handlers: EditorBarHandlers }) {
  const run = useQueryTabField(tabId, "run");
  const plan = useQueryTabField(tabId, "plan");
  const resultsCollapsed = useQueryTabField(tabId, "resultsCollapsed");
  const busy = run?.status === "running" || plan?.status === "running";
  const cancelling = run?.status === "running" && run.cancelling;
  // What the session offers (unknown: everything, as for the core sessions).
  const connectionId = useTabsStore((s) => s.tabs.find((t) => t.id === tabId)?.connectionId);
  const caps = useCaps(connectionId ?? null);
  const offers = (f: Parameters<typeof has>[1]) => caps === null || has(caps, f);

  return (
    <div
      role="toolbar"
      aria-label="Query"
      className="border-border bg-surface-2 @container shrink-0 border-t"
    >
      <div className="flex min-h-10 flex-wrap items-center gap-x-2 gap-y-1 px-2 py-1 @max-[440px]:gap-x-1">
        <div className="flex min-w-0 items-center gap-1">
          <EditorSettingsMenu />
          <IconButton
            icon={resultsCollapsed ? PanelBottomOpen : PanelBottomClose}
            label={resultsCollapsed ? "Show results" : "Hide results"}
            action="query.toggle-results"
            onClick={handlers.toggleResults}
          />
          <CursorPosition tabId={tabId} />
          <Divider className="@max-[560px]:hidden" />
          <FileActions handlers={handlers} />
        </div>
        <Divider className="@max-[440px]:hidden" />
        {/* On the narrowest panes the outcome of the run takes a row of its own. */}
        <div className="flex min-w-0 flex-1 items-center empty:hidden @max-[440px]:order-last @max-[440px]:basis-full">
          <RunSummary tabId={tabId} />
        </div>
        <div className="ml-auto flex min-w-0 flex-wrap items-center justify-end gap-1">
          <RowLimitSelect tabId={tabId} />
          {offers("format") && <BeautifyButton handlers={handlers} />}
          {offers("explain") && (
            <ExplainButton
              handlers={handlers}
              busy={busy}
              analyze={offers("explainAnalyze")}
              rollsBack={caps === null || !appliesPartially(caps.data.transactions)}
            />
          )}
          <RunButton handlers={handlers} busy={busy} cancelling={cancelling} />
        </div>
      </div>
    </div>
  );
}
