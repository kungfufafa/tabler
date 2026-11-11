import { useQueryClient } from "@tanstack/react-query";
import {
  ChartColumn,
  ChartLine,
  ChartPie,
  Pin,
  Rows3,
  Search,
  Settings2,
  Table2,
  TriangleAlert,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, type KeyboardEvent, type ReactNode } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Kbd } from "@/components/Kbd";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ChartView } from "@/features/charts/ChartView";
import type { ChartDirective, ChartKind } from "@/features/charts/directive";
import { directiveFor, setResultView, useResultView } from "@/features/charts/store";
import { ROW_H } from "@/features/datagrid/types";
import { openExport } from "@/features/export";
import { resultTable } from "@/features/export/resultTable";
import { useAction } from "@/features/palette/actions";
import { redoResultEdit, setResultRerun } from "@/features/results/commit";
import { appendedHeader, pinnedLabel, resultTabText } from "@/features/results/footer";
import {
  dirtyResultOf,
  installResultsLifecycle,
  isPinId,
  pinResult,
  pinsOf,
  stagingOf,
  unpinResult,
  useResultsStore,
  type PinnedResult,
} from "@/features/results/store";
import { canRedo } from "@/features/staging/model";
import type { Dialect } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { setPref, usePrefs, type ResultViewMode } from "@/stores/prefs";
import { getQueryTab, updateQueryTab, useQueryTabField, type ResultPane } from "@/stores/queryTabs";
import { resultData, useResultRowCount } from "@/stores/resultData";
import { useTabsStore } from "@/stores/tabs";
import { exportQuery, runQuery } from "./controller";
import { HISTORY_KEY } from "./historyGroups";
import { MessagesView } from "./MessagesView";
import { installNoticeNotifications } from "./notices";
import { resolvePane } from "./panes";
import { PlanView } from "./PlanView";
import { ResultFooter, ResultGrid } from "./ResultSetGrid";
import { summarize, type ResultSetInfo, type RunState } from "./runModel";
import { closeFind, installFindLifecycle, openFind, useFind } from "./search/store";

installResultsLifecycle();
installNoticeNotifications();
installFindLifecycle();

/** The chrome around a stacked result set (the rows are `ROW_H` high: a live value, Appearance). */
const STACK_CHROME_H = 24 + 28;
/** Rows a stacked result set shows before it scrolls on its own. */
const STACK_MAX_ROWS = 12;
const STACK_MIN_ROWS = 3;

/** One segment of the results strip (or of the Chart | Grid switch: `role` radio). */
function PaneTab({
  active,
  onSelect,
  children,
  title,
  role = "tab",
}: {
  active: boolean;
  onSelect: () => void;
  children: ReactNode;
  title?: string;
  role?: "tab" | "radio";
}) {
  return (
    <button
      type="button"
      role={role}
      aria-selected={role === "tab" ? active : undefined}
      aria-checked={role === "radio" ? active : undefined}
      tabIndex={active || role === "radio" ? 0 : -1}
      title={title}
      onClick={onSelect}
      className={cn(
        "flex h-full shrink-0 items-center gap-2 px-2 text-[12px] whitespace-nowrap transition-colors outline-none",
        // Tailwind v4: `outline-none` sets --tw-outline-style:none, so the ring needs its style.
        "focus-visible:outline-ring/70 focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-solid",
        role === "tab" ? "relative" : "rounded-sm",
        active
          ? cn(
              "text-foreground font-medium",
              role === "tab"
                ? "after:bg-brand after:pointer-events-none after:absolute after:inset-x-0 after:bottom-0 after:h-0.5"
                : "bg-accent",
            )
          : "text-muted-foreground hover:text-foreground hover:bg-row-hover",
      )}
    >
      {children}
    </button>
  );
}

function ResultTab({
  info,
  label,
  active,
  pinned = false,
  onSelect,
}: {
  info: ResultSetInfo;
  label: string;
  active: boolean;
  pinned?: boolean;
  onSelect: () => void;
}) {
  const rows = useResultRowCount(info.id);
  const pending = useResultsStore((s) => stagingOf(s, info.id).changes.size);
  return (
    <PaneTab active={active} onSelect={onSelect} title={pinned ? "Pinned result" : undefined}>
      {pinned && <Pin className="size-3.5" aria-label="Pinned" />}
      <span className="tabular-nums">{resultTabText(label, rows, info.durationMs)}</span>
      {info.truncated && <TriangleAlert className="text-warning size-3" aria-label="Truncated" />}
      {pending > 0 && (
        <span
          aria-label={`${pending} pending`}
          className="bg-brand size-1.5 shrink-0 rounded-full"
          data-slot="result-pending"
        />
      )}
    </PaneTab>
  );
}

function ResultsEmpty() {
  const shortcuts = [
    { label: "Run current", combo: shortcutFor("query.run-current") },
    { label: "Run all", combo: shortcutFor("query.run") },
    { label: "Explain", combo: shortcutFor("query.explain") },
  ].flatMap(({ label, combo }) =>
    combo
      ? [
          <li key={label} className="flex items-center gap-1 whitespace-nowrap">
            {label}
            <Kbd combo={combo} />
          </li>,
        ]
      : [],
  );
  return (
    <EmptyState
      title="Run a query"
      description="Results appear here after a statement runs"
      className="justify-center-safe overflow-y-auto p-4"
    >
      {shortcuts.length > 0 && (
        <ul className="text-muted-foreground flex flex-wrap justify-center gap-x-4 gap-y-2 text-[12px]">
          {shortcuts}
        </ul>
      )}
    </EmptyState>
  );
}

/** Results header ▸ Result View Mode, row numbers and Pin Result. */
function ResultsMenu({
  mode,
  pinTarget,
  onTogglePin,
}: {
  mode: ResultViewMode;
  /** The result Pin Result acts on, and whether it is pinned. */
  pinTarget: { pinned: boolean } | null;
  onTogglePin: () => void;
}) {
  const rowNumbers = usePrefs((p) => p.resultRowNumbers);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Result options"
        title="Result options"
        className="text-muted-foreground hover:text-foreground hover:bg-row-hover focus-visible:ring-ring/50 flex size-7 shrink-0 items-center justify-center rounded-md transition-colors outline-none focus-visible:ring-2"
      >
        <Settings2 className="size-4" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-60">
        <DropdownMenuLabel className="text-muted-foreground text-[11px]">
          Result View Mode
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={mode}
          onValueChange={(v) => setPref("resultViewMode", v as ResultViewMode)}
        >
          <DropdownMenuRadioItem value="tabs">Split Results into Tabs</DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="append">
            Append All Results in One Tab
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuCheckboxItem
          checked={rowNumbers}
          onCheckedChange={(checked) => setPref("resultRowNumbers", checked === true)}
        >
          Show Row Numbers
        </DropdownMenuCheckboxItem>
        <DropdownMenuItem disabled={!pinTarget} onSelect={onTogglePin}>
          {pinTarget?.pinned ? "Unpin Result" : "Pin Result"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** What a result shown in the pane is: a set of the current run, or a pinned copy. */
interface Shown {
  info: ResultSetInfo;
  label: string;
  /** Set when it is a pinned copy. */
  pin: PinnedResult | null;
  sql: string | null;
  /** The chart its statement's comment asks for (`-- TablePlus BarChart x: …, y: …`). */
  directive: ChartDirective | null;
}

const CHART_ICONS: Record<ChartKind, typeof ChartColumn> = {
  bar: ChartColumn,
  line: ChartLine,
  pie: ChartPie,
};

/** The results header's Chart | Grid switch of a result with a chart directive. */
function ViewSwitch({ resultId, kind }: { resultId: string; kind: ChartKind }) {
  const view = useResultView(resultId, true);
  const ChartIcon = CHART_ICONS[kind];
  return (
    <div
      role="radiogroup"
      aria-label="Result view"
      className="border-border bg-surface-2 flex h-7 shrink-0 items-stretch rounded-md border p-0.5"
    >
      <PaneTab
        role="radio"
        active={view === "chart"}
        onSelect={() => setResultView(resultId, "chart")}
      >
        <ChartIcon className="size-3.5" />
        Chart
      </PaneTab>
      <PaneTab
        role="radio"
        active={view === "grid"}
        onSelect={() => setResultView(resultId, "grid")}
      >
        <Table2 className="size-3.5" />
        Grid
      </PaneTab>
    </div>
  );
}

/** A result's body: its chart (when its comment asks for one and the switch says so) or grid. */
function ResultBody({
  shown,
  height,
  footer,
  ...grid
}: {
  shown: Shown;
  tabId: string;
  connectionId: string | null;
  dialect: Dialect | null;
  active: boolean;
  height?: number;
  footer: ReactNode;
  /** The grid menu's Export Result…. */
  onExport: () => void;
}) {
  const view = useResultView(shown.info.id, shown.directive !== null);
  if (view === "chart" && shown.directive)
    return (
      <div
        className="flex min-h-0 flex-col"
        style={height === undefined ? { height: "100%" } : { height }}
      >
        <div className="min-h-0 flex-1">
          <ChartView
            resultId={shown.info.id}
            columns={shown.info.columns}
            directive={shown.directive}
            done={shown.info.done}
          />
        </div>
        {footer}
      </div>
    );
  return (
    <ResultGrid
      {...grid}
      info={shown.info}
      label={shown.label}
      sql={shown.sql}
      height={height}
      footer={footer}
    />
  );
}

function statementSql(run: RunState | null, info: ResultSetInfo): string | null {
  return run?.statements.find((s) => s.index === info.statementIndex)?.sql ?? null;
}

/**
 * Result sets, messages and plan of the tab's latest run, below the editor bar: a compact
 * segmented strip (pinned results, Result 1 / Result 2 or all of them in one "Results" tab,
 * Messages, Plan) over the selected pane, which has TablePlus's footer: row count, time, Show
 * Message, Pin Result, row numbers and Export…. Results whose columns come from one table with
 * its primary key are editable; ⌘S commits their staged changes.
 */
export function ResultsPane({ tabId, dialect }: { tabId: string; dialect: Dialect | null }) {
  const qc = useQueryClient();
  const run = useQueryTabField(tabId, "run");
  const plan = useQueryTabField(tabId, "plan");
  const pane = useQueryTabField(tabId, "pane");
  const isActive = useTabsStore((s) => s.activeId === tabId);
  const connectionId = useTabsStore((s) => {
    const tab = s.tabs.find((t) => t.id === tabId);
    return tab?.kind === "query" ? tab.connectionId : null;
  });
  const mode = usePrefs((p) => p.resultViewMode);
  const pins = useResultsStore((s) => pinsOf(s, tabId));
  const dirty = useResultsStore((s) => dirtyResultOf(s, tabId));
  const redoable = useResultsStore((s) => (dirty ? canRedo(stagingOf(s, dirty)) : false));
  const findOpen = useFind(tabId).open;

  // "Re-run" after a commit runs the result's statement again in this tab.
  useEffect(() => {
    setResultRerun(
      (id, sub) =>
        void runQuery(id, sub, {
          onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
        }),
    );
  }, [qc]);

  // Staged result edits are part of the workspace commit (⌘S / ⇧⌘P / ⇧⌘⌫ commit, preview and
  // discard every pending change of the connection, `features/commit`); ⇧⌘Z redoes here.
  const redo = useCallback(() => {
    if (dirty) redoResultEdit(tabId, dirty);
  }, [tabId, dirty]);
  useAction("grid.redo", isActive && redoable ? redo : null);

  // ⌘F in a result grid (or the header's search icon): find & replace in the shown result.
  const findTarget = useRef<Shown | null>(null);
  const openFindHere = useCallback(() => {
    const target = findTarget.current;
    if (!target) return;
    if (target.directive) setResultView(target.info.id, "grid");
    openFind(tabId);
  }, [tabId]);
  useAction("grid.filter", isActive ? openFindHere : null);

  const pinnedSources = useMemo(() => new Set(pins.map((p) => p.sourceId)), [pins]);
  const results = useMemo(
    () => (run?.results ?? []).filter((r) => !pinnedSources.has(r.id)),
    [run, pinnedSources],
  );
  if (!run && !plan && pins.length === 0) return <ResultsEmpty />;

  const runIndex = (info: ResultSetInfo) => (run?.results.indexOf(info) ?? 0) + 1;
  const shownOf = (id: string): Shown | null => {
    const pin = pins.find((p) => p.id === id);
    if (pin)
      return {
        info: pin.info,
        label: pinnedLabel(pin.label),
        pin,
        sql: pin.sql,
        directive: directiveFor(pin.id, { pinnedFrom: pin.sourceId }),
      };
    const info = results.find((r) => r.id === id);
    return info && run
      ? {
          info,
          label: `Result ${runIndex(info)}`,
          pin: null,
          sql: statementSql(run, info),
          directive: directiveFor(info.id, { tabId, run, statementIndex: info.statementIndex }),
        }
      : null;
  };

  let active: ResultPane =
    isPinId(pane) && pins.some((p) => p.id === pane) ? pane : resolvePane(pane, results, plan);
  if (active === "messages" && !run) active = pins[0]?.id ?? "messages";
  const append = mode === "append" && results.length > 0;
  const activeResult = results.find((r) => r.id === active);
  /** The strip's tabs, in order (the stacked results are one tab: their first set's id). */
  const panes: ResultPane[] = [
    ...pins.map((p) => p.id),
    ...(append ? [results[0]!.id] : results.map((r) => r.id)),
    ...(run ? (["messages"] as const) : []),
    ...(plan ? (["plan"] as const) : []),
  ];
  const activeTab: ResultPane = append && activeResult ? results[0]!.id : active;
  const select = (p: ResultPane) => updateQueryTab(tabId, { pane: p });
  const errors = run ? summarize(run).errors : 0;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const i = panes.indexOf(activeTab);
    const next = panes[(i + (e.key === "ArrowRight" ? 1 : -1) + panes.length) % panes.length];
    if (next === undefined) return;
    e.preventDefault();
    select(next);
    const tabs = e.currentTarget.querySelectorAll<HTMLElement>("[role=tab]");
    requestAnimationFrame(() => tabs[panes.indexOf(next)]?.focus());
  };

  const togglePin = (shown: Shown) => {
    if (shown.pin) {
      unpinResult(tabId, shown.pin.id);
      return;
    }
    const id = pinResult(tabId, shown.info, shown.label, shown.sql);
    if (id && getQueryTab(tabId).pane === shown.info.id) select(id);
  };
  const exportShown = (shown: Shown) => {
    if (!shown.pin) {
      // The controller exports the result the pane shows (as ⇧⌘E does).
      select(shown.info.id);
      exportQuery(tabId);
      return;
    }
    const buffer = resultData.get(shown.info.id);
    if (!buffer || !connectionId) return;
    const columns = buffer.columns.length > 0 ? buffer.columns : shown.info.columns;
    openExport({
      connectionId,
      source: {
        kind: "rows",
        columns: columns.map((c) => ({ ...c })),
        rows: buffer.rows.map((row) => [...row]),
        label: shown.label,
        table: resultTable(columns, shown.sql),
        sql: shown.info.set === 0 ? shown.sql : null,
        truncated: shown.info.truncated,
        stopped: null,
      },
    });
  };

  const gridFor = (shown: Shown, gridActive: boolean, height?: number) => (
    <ResultBody
      key={shown.info.id}
      shown={shown}
      tabId={tabId}
      connectionId={connectionId}
      dialect={dialect}
      active={gridActive}
      height={height}
      onExport={() => exportShown(shown)}
      footer={
        <FooterFor
          shown={shown}
          onShowMessage={run ? () => select("messages") : null}
          onTogglePin={() => togglePin(shown)}
          onExport={() => exportShown(shown)}
        />
      }
    />
  );

  const activeShown = typeof active === "string" ? shownOf(active) : null;
  findTarget.current = activeShown;

  return (
    <div className="bg-surface flex h-full min-h-0 flex-col">
      <div className="border-border flex h-9 shrink-0 items-center gap-2 border-b px-2">
        <div
          role="tablist"
          aria-label="Results"
          className="flex h-full min-w-0 [scrollbar-width:none] items-stretch gap-1 overflow-x-auto"
          onKeyDown={onKeyDown}
        >
          {pins.map((p) => (
            <ResultTab
              key={p.id}
              info={p.info}
              label={p.label}
              pinned
              active={active === p.id}
              onSelect={() => select(p.id)}
            />
          ))}
          {append ? (
            <PaneTab
              active={!!activeResult}
              onSelect={() => select(activeResult ? activeResult.id : results[0]!.id)}
            >
              <Rows3 className="size-3.5" />
              Results
              <span className="text-muted-foreground tabular-nums">{results.length}</span>
            </PaneTab>
          ) : (
            results.map((r) => (
              <ResultTab
                key={r.id}
                info={r}
                label={`Result ${runIndex(r)}`}
                active={active === r.id}
                onSelect={() => select(r.id)}
              />
            ))
          )}
          {run && (
            <PaneTab active={active === "messages"} onSelect={() => select("messages")}>
              Messages
              {errors > 0 && <span className="text-error text-[12px] tabular-nums">{errors}</span>}
            </PaneTab>
          )}
          {plan && (
            <PaneTab active={active === "plan"} onSelect={() => select("plan")}>
              Plan
            </PaneTab>
          )}
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {activeShown?.directive && (
            <ViewSwitch resultId={activeShown.info.id} kind={activeShown.directive.kind} />
          )}
          <button
            type="button"
            aria-label="Find in Result"
            title={`Find in Result (${formatCombo(shortcutFor("grid.filter") ?? "Mod+F")})`}
            aria-pressed={findOpen}
            disabled={!activeShown}
            onClick={() => (findOpen ? closeFind(tabId) : openFindHere())}
            className={cn(
              "text-muted-foreground hover:text-foreground hover:bg-row-hover focus-visible:ring-ring/50 flex size-7 shrink-0 items-center justify-center rounded-md transition-colors outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-40",
              findOpen && "bg-muted text-foreground",
            )}
          >
            <Search className="size-4" />
          </button>
          <ResultsMenu
            mode={mode}
            pinTarget={activeShown ? { pinned: activeShown.pin !== null } : null}
            onTogglePin={() => activeShown && togglePin(activeShown)}
          />
        </div>
      </div>
      <div className="min-h-0 flex-1" role="tabpanel">
        {active === "plan" && plan ? (
          <PlanView tabId={tabId} plan={plan} dialect={dialect} />
        ) : append && activeResult ? (
          <div className="h-full overflow-y-auto" data-slot="appended-results">
            {results.map((r) => {
              const shown = shownOf(r.id)!;
              return (
                <AppendedResult
                  key={r.id}
                  n={runIndex(r)}
                  info={r}
                  selected={r.id === active}
                  onSelect={() => r.id !== getQueryTab(tabId).pane && select(r.id)}
                >
                  {(height) => gridFor(shown, isActive && r.id === active, height)}
                </AppendedResult>
              );
            })}
          </div>
        ) : activeShown ? (
          gridFor(activeShown, isActive)
        ) : run ? (
          <MessagesView tabId={tabId} run={run} />
        ) : null}
      </div>
    </div>
  );
}

/** One result set of "Append All Results in One Tab": a header, then its grid and footer. */
function AppendedResult({
  n,
  info,
  selected,
  onSelect,
  children,
}: {
  n: number;
  info: ResultSetInfo;
  selected: boolean;
  onSelect: () => void;
  children: (height: number) => ReactNode;
}) {
  const rows = useResultRowCount(info.id);
  const pending = useResultsStore((s) => stagingOf(s, info.id).changes.size);
  const shownRows = Math.min(STACK_MAX_ROWS, Math.max(STACK_MIN_ROWS, rows + pending));
  return (
    <section
      aria-label={`Result ${n}`}
      data-selected={selected || undefined}
      className="border-border border-b last:border-b-0"
      onPointerDownCapture={onSelect}
      onFocusCapture={onSelect}
    >
      <header
        className={cn(
          "bg-surface-2 text-muted-foreground border-border flex h-6 items-center gap-2 border-b px-3 text-[12px] tabular-nums",
          selected && "text-foreground font-medium",
        )}
      >
        {appendedHeader(n, rows, info.durationMs)}
        {info.truncated && <TriangleAlert className="text-warning size-3" aria-label="Truncated" />}
      </header>
      {children(STACK_CHROME_H + (shownRows + 1) * ROW_H)}
    </section>
  );
}

function FooterFor({
  shown,
  onShowMessage,
  onTogglePin,
  onExport,
}: {
  shown: Shown;
  onShowMessage: (() => void) | null;
  onTogglePin: () => void;
  onExport: () => void;
}) {
  const rows = useResultRowCount(shown.info.id);
  return (
    <ResultFooter
      info={shown.info}
      rows={rows}
      pinned={shown.pin !== null}
      onShowMessage={onShowMessage}
      onTogglePin={onTogglePin}
      onExport={onExport}
    />
  );
}
