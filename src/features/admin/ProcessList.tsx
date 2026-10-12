import { useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Loader2, RefreshCw, Server, TriangleAlert } from "lucide-react";
import { useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type Driver, type KillScope, type ProcessRow } from "@/lib/ipc";
import { isMac } from "@/lib/keys/combo";
import { cn } from "@/lib/utils";
import { adminKeys, PROCESS_REFRESH_MS } from "./keys";
import { NativeSelect } from "./parts";
import {
  cellText,
  clickSelection,
  filterProcesses,
  killSummary,
  processColumns,
  processDatabases,
  sortProcesses,
  type ProcessColumnId,
  type ProcessSort,
} from "./processes";
import { errorInfo } from "./run";

/** Connection ▸ Process List… (⌘.): the server's sessions, with Kill Query / Kill Connection. */
export function ProcessListDialog({
  connectionId,
  driver,
  connectionName,
  onClose,
}: {
  connectionId: string;
  driver: Driver;
  connectionName: string;
  onClose: () => void;
}) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex h-[min(680px,80vh)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(1120px,calc(100%-2rem))]">
        <header className="border-border flex h-11 shrink-0 items-center gap-2 border-b pr-12 pl-4">
          <DialogTitle className="text-[13px] font-semibold">Running Queries</DialogTitle>
          <DialogDescription className="text-muted-foreground truncate text-xs">
            {connectionName}
          </DialogDescription>
        </header>
        {driver === "sqlite" ? (
          <EmptyState
            icon={Server}
            title="Process list is not available"
            description="SQLite runs inside Tabler: it has no server sessions to list or kill."
          />
        ) : (
          <ProcessListBody connectionId={connectionId} driver={driver} />
        )}
      </DialogContent>
    </Dialog>
  );
}

function ProcessListBody({ connectionId, driver }: { connectionId: string; driver: Driver }) {
  const [auto, setAuto] = useState(false);
  const [hideIdle, setHideIdle] = useState(false);
  const [database, setDatabase] = useState<string | null>(null);
  const [sort, setSort] = useState<ProcessSort>({ column: "id", desc: false });
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<KillScope | null>(null);
  const [killing, setKilling] = useState(false);

  const list = useQuery({
    queryKey: adminKeys.processes(connectionId),
    queryFn: () => ipc.admin.processes(connectionId),
    refetchInterval: auto ? PROCESS_REFRESH_MS : false,
    refetchOnWindowFocus: false,
    gcTime: 0,
    retry: false,
  });
  const columns = processColumns(driver);
  const all = useMemo(() => list.data ?? [], [list.data]);
  const databases = useMemo(() => processDatabases(all), [all]);
  const rows = useMemo(
    () => sortProcesses(filterProcesses(all, { database, hideIdle }), sort),
    [all, database, hideIdle, sort],
  );
  const ids = useMemo(() => rows.map((r) => r.id), [rows]);
  // Sessions that ended (or were filtered out) drop out of the selection.
  const picked = rows.filter((r) => selected.has(r.id));
  const focused = rows.find((r) => r.id === anchor) ?? null;

  const onRowClick = (e: MouseEvent, id: number) => {
    const toggle = isMac ? e.metaKey : e.ctrlKey;
    setSelected(clickSelection(ids, selected, anchor, id, { toggle, range: e.shiftKey }));
    if (!e.shiftKey) setAnchor(id);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      setSelected(new Set(ids));
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const at = anchor === null ? -1 : ids.indexOf(anchor);
    const next = ids[Math.max(0, Math.min(ids.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)))];
    if (next === undefined) return;
    setAnchor(next);
    setSelected(new Set([next]));
  };

  const toggleSort = (column: ProcessColumnId) =>
    setSort((s) => (s.column === column ? { column, desc: !s.desc } : { column, desc: false }));

  const kill = async (scope: KillScope) => {
    const targets = picked.map((r) => r.id);
    if (targets.length === 0 || killing) return;
    setConfirm(null);
    const label = `Kill ${scope === "query" ? "the query" : "the connection"} of ${
      targets.length === 1 ? `process ${targets[0]}` : `${targets.length} processes`
    }`;
    if (!(await guardExecution(connectionId, { kind: "operation", label, write: true }))) return;
    setKilling(true);
    try {
      const outcomes = await ipc.admin.kill(connectionId, targets, scope);
      const summary = killSummary(outcomes, scope);
      const description = summary.failed.map((f) => `${f.id}: ${f.message}`).join("\n");
      if (summary.failed.length === 0) toast.success(summary.title);
      else toast.error(summary.title, { description });
    } catch (e) {
      toast.error("Could not kill", { description: errorInfo(e).message });
    } finally {
      setKilling(false);
      void list.refetch();
    }
  };

  return (
    <>
      <div className="border-border flex h-10 shrink-0 items-center gap-3 border-b px-3 text-xs">
        <label className="flex items-center gap-1.5">
          <span className="text-muted-foreground">Database</span>
          <NativeSelect
            aria-label="Database filter"
            value={database ?? ""}
            onChange={(e) => setDatabase(e.target.value === "" ? null : e.target.value)}
            className="w-40"
          >
            <option value="">All databases</option>
            {databases.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </NativeSelect>
        </label>
        <label className="flex items-center gap-1.5">
          <Checkbox checked={hideIdle} onCheckedChange={(v) => setHideIdle(v === true)} />
          Hide idle
        </label>
        <label className="flex items-center gap-1.5">
          <Checkbox checked={auto} onCheckedChange={(v) => setAuto(v === true)} />
          Auto-refresh (2 s)
        </label>
        <span className="flex-1" />
        <Button
          variant="outline"
          size="xs"
          onClick={() => void list.refetch()}
          disabled={list.isFetching}
        >
          <RefreshCw className={cn(list.isFetching && "animate-spin")} />
          Refresh
        </Button>
      </div>

      <div
        role="grid"
        aria-label="Processes"
        aria-multiselectable
        tabIndex={0}
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-auto outline-none"
      >
        {list.isError ? (
          <EmptyState
            icon={TriangleAlert}
            title="Could not list the processes"
            description={errorInfo(list.error).message}
          >
            <Button variant="outline" size="xs" onClick={() => void list.refetch()}>
              Try again
            </Button>
          </EmptyState>
        ) : list.isPending ? (
          <div className="text-muted-foreground flex h-full items-center justify-center gap-2 text-xs">
            <Loader2 className="size-3.5 animate-spin" /> Loading processes…
          </div>
        ) : (
          <table className="w-full min-w-[900px] table-fixed border-collapse text-xs">
            <colgroup>
              {columns.map((c) => (
                <col key={c.id} className={c.width} />
              ))}
            </colgroup>
            <thead>
              <tr className="text-muted-foreground [&>th]:bg-surface-2 h-7 text-left text-[11px] [&>th]:sticky [&>th]:top-0 [&>th]:z-10 [&>th]:shadow-[inset_0_-1px_0_var(--color-border)]">
                {columns.map((c) => (
                  <th
                    key={c.id}
                    aria-sort={
                      sort.column === c.id ? (sort.desc ? "descending" : "ascending") : "none"
                    }
                    className="px-0 font-medium"
                  >
                    <button
                      type="button"
                      onClick={() => toggleSort(c.id)}
                      className={cn(
                        "hover:text-foreground flex h-7 w-full items-center gap-1 px-2 text-left",
                        c.numeric && "justify-end",
                      )}
                    >
                      {c.label}
                      {sort.column === c.id &&
                        (sort.desc ? (
                          <ArrowDown className="size-3" />
                        ) : (
                          <ArrowUp className="size-3" />
                        ))}
                    </button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <ProcessTableRow
                  key={r.id}
                  row={r}
                  columns={columns.map((c) => c.id)}
                  numeric={columns.filter((c) => c.numeric).map((c) => c.id)}
                  selected={selected.has(r.id)}
                  onClick={(e) => onRowClick(e, r.id)}
                />
              ))}
              {rows.length === 0 && (
                <tr>
                  <td
                    colSpan={columns.length}
                    className="text-muted-foreground px-3 py-6 text-center text-[11px]"
                  >
                    {all.length === 0 ? "No sessions" : "No sessions match the filter"}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        )}
      </div>

      {focused?.query && (
        <div className="border-border bg-surface-2 max-h-24 shrink-0 overflow-auto border-t px-3 py-2">
          <pre className="selectable font-mono text-[11px] break-words whitespace-pre-wrap">
            {focused.query}
          </pre>
        </div>
      )}

      <footer className="border-border flex h-11 shrink-0 items-center gap-2 border-t px-3 text-xs">
        <span className="text-muted-foreground tabular-nums">
          {rows.length} {rows.length === 1 ? "session" : "sessions"}
          {picked.length > 0 && ` · ${picked.length} selected`}
        </span>
        <span className="flex-1" />
        <Button
          variant="outline"
          size="sm"
          disabled={picked.length === 0 || killing}
          onClick={() => setConfirm("query")}
        >
          Kill Query
        </Button>
        <Button
          variant="destructive"
          size="sm"
          disabled={picked.length === 0 || killing}
          onClick={() => setConfirm("connection")}
        >
          {killing && <Loader2 className="animate-spin" />}
          Kill Connection
        </Button>
      </footer>

      {confirm && (
        <KillConfirm
          driver={driver}
          scope={confirm}
          targets={picked}
          onCancel={() => setConfirm(null)}
          onConfirm={() => void kill(confirm)}
        />
      )}
    </>
  );
}

function ProcessTableRow({
  row,
  columns,
  numeric,
  selected,
  onClick,
}: {
  row: ProcessRow;
  columns: ProcessColumnId[];
  numeric: ProcessColumnId[];
  selected: boolean;
  onClick: (e: MouseEvent) => void;
}) {
  return (
    <tr
      role="row"
      aria-selected={selected}
      data-process={row.id}
      onMouseDown={(e) => {
        // ⇧-click extends the selection; keep the browser from selecting text.
        if (e.shiftKey) e.preventDefault();
      }}
      onClick={onClick}
      className={cn(
        "border-gridline h-7 cursor-default border-b select-none",
        selected ? "bg-brand-soft" : "hover:bg-muted/50",
      )}
    >
      {columns.map((c) => {
        const text = cellText(row, c);
        return (
          <td
            key={c}
            title={
              c === "query"
                ? (row.query ?? undefined)
                : c === "duration"
                  ? (row.startedAt ?? undefined)
                  : text
            }
            className={cn(
              "truncate px-2",
              numeric.includes(c) && "text-right tabular-nums",
              (c === "query" || c === "id") && "font-mono text-[11px]",
              c === "query" && !text && "text-muted-foreground/60",
            )}
          >
            {c === "id" && row.own && (
              <span
                className="bg-brand-soft text-brand mr-1 rounded px-1 font-sans text-[10px] font-medium"
                title={
                  row.current
                    ? "The session listing these processes"
                    : "One of Tabler's own sessions"
                }
              >
                Tabler
              </span>
            )}
            {text || (c === "query" ? "—" : "")}
          </td>
        );
      })}
    </tr>
  );
}

const KILL_TEXT: Record<Driver, Record<KillScope, string>> = {
  postgres: {
    query:
      "pg_cancel_backend asks each session to stop its running statement; the sessions stay connected.",
    connection:
      "pg_terminate_backend ends each session; whatever its open transaction did is rolled back.",
  },
  mysql: {
    query: "KILL QUERY stops each thread's running statement; the connections stay open.",
    connection: "KILL closes each connection; whatever its open transaction did is rolled back.",
  },
  sqlite: { query: "", connection: "" },
};

function KillConfirm({
  driver,
  scope,
  targets,
  onCancel,
  onConfirm,
}: {
  driver: Driver;
  scope: KillScope;
  targets: ProcessRow[];
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const own = targets.filter((t) => t.own);
  const what = scope === "query" ? "query" : "connection";
  const title =
    targets.length === 1
      ? `Kill the ${what} of process ${targets[0]?.id}?`
      : `Kill the ${what === "query" ? "queries" : "connections"} of ${targets.length} processes?`;
  return (
    <AlertDialog open onOpenChange={(open) => !open && onCancel()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">{title}</AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            {KILL_TEXT[driver][scope]}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {own.length > 0 && (
          <p
            role="alert"
            className="border-warning/40 bg-warning/10 flex gap-2 rounded-md border px-2.5 py-2 text-xs"
          >
            <TriangleAlert className="text-warning mt-0.5 size-3.5 shrink-0" />
            <span>
              {own.length === targets.length && own.length === 1
                ? "This is "
                : `${own.length} of them ${own.length === 1 ? "is" : "are"} `}
              {driver === "mysql" ? "probably " : ""}
              Tabler&apos;s own {own.length === 1 ? "session" : "sessions"} (browsing, the SQL
              editor or this list). Killing {own.length === 1 ? "it" : "them"} interrupts Tabler,
              which reconnects on its next query.
            </span>
          </p>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
          <Button variant="destructive" size="sm" autoFocus onClick={onConfirm}>
            {scope === "query" ? "Kill Query" : "Kill Connection"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
