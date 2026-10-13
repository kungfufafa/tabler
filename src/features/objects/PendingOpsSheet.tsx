import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Eraser, Loader2, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { actions } from "@/features/palette/actions";
import { SqlBlock } from "@/features/structure/SqlBlock";
import { useStructureStore } from "@/features/structure/store";
import { ipc, type DdlOp, type Dialect, type TableRef } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { useActiveTab, type Tab } from "@/stores/tabs";
import { closePendingSheet, runObjectCommit, usePendingSheetStore } from "./pendingActions";
import {
  describeOp,
  unstageObjectOp,
  useObjectOpsCommitting,
  usePendingObjectOps,
  type PendingObjectOp,
} from "./pendingOps";
import { useTabPendingCount } from "./tabPending";

const NO_OPS: DdlOp[] = [];

function sqlOf(statements: readonly string[]): string {
  return statements.map((s) => (s.trimEnd().endsWith(";") ? s.trimEnd() : `${s};`)).join("\n");
}

/** The active tab's part: its structure SQL, or a pointer to its own Preview SQL sheet. */
function TabSection({ tab, count, dialect }: { tab: Tab; count: number; dialect: Dialect }) {
  const ops = useStructureStore((s) =>
    tab.kind === "structure" ? (s.byTab[tab.id]?.ops ?? NO_OPS) : NO_OPS,
  );
  const table: TableRef | null = tab.kind === "query" ? null : tab.table;
  const plan = useQuery({
    queryKey: ["pendingTabPreview", tab.connectionId, tab.id, ops],
    queryFn: () => ipc.ddl.preview(tab.connectionId, table, ops),
    enabled: tab.kind === "structure" && ops.length > 0,
    retry: false,
    staleTime: Infinity,
  });
  const changes = `${count} staged change${count === 1 ? "" : "s"}`;
  return (
    <section aria-label={`Changes in ${tab.title}`} className="flex flex-col gap-1.5">
      <h3 className="text-muted-foreground flex items-center gap-2 text-[11px] font-semibold">
        <span className="truncate">
          {tab.title} · {changes}
        </span>
        {tab.kind !== "structure" && (
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-6 px-2 text-[11px]"
            onClick={() => {
              closePendingSheet();
              void actions.run("grid.preview");
            }}
          >
            Preview…
          </Button>
        )}
      </h3>
      <div className="border-border bg-surface-2 rounded-md border px-3 py-2">
        {tab.kind !== "structure" ? (
          <p className="text-muted-foreground text-[11px]">
            Committed first, in their own transaction.
          </p>
        ) : plan.data ? (
          <SqlBlock sql={sqlOf(plan.data.statements)} dialect={dialect} />
        ) : plan.isError ? (
          <p className="text-error text-xs">{plan.error.message}</p>
        ) : (
          <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
        )}
      </div>
    </section>
  );
}

function OpItem({
  op,
  dialect,
  disabled,
  onRemove,
}: {
  op: PendingObjectOp;
  dialect: Dialect;
  /** While the operations are being committed. */
  disabled: boolean;
  onRemove: () => void;
}) {
  const { verb, what } = describeOp(op.op);
  const Icon = op.kind === "drop" ? Trash2 : Eraser;
  return (
    <li
      data-pending-op={op.kind}
      className="border-border bg-surface-2 flex flex-col gap-1 rounded-md border px-3 py-2"
    >
      <div className="flex items-center gap-1.5 text-[11px] font-medium">
        <Icon className="text-muted-foreground size-3.5 shrink-0" />
        <span className="truncate">
          {verb} {what}
        </span>
        <button
          type="button"
          aria-label={`Remove ${verb.toLowerCase()} of ${what}`}
          onClick={onRemove}
          disabled={disabled}
          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 ml-auto rounded-sm outline-none focus-visible:ring-2 disabled:pointer-events-none disabled:opacity-40"
        >
          <X className="size-3.5" />
        </button>
      </div>
      <SqlBlock sql={sqlOf(op.statements)} dialect={dialect} />
      {op.error && (
        <p
          role="alert"
          className="text-error flex items-start gap-1.5 text-[11px] break-words whitespace-pre-wrap"
        >
          <AlertTriangle className="mt-px size-3.5 shrink-0" />
          {op.error}
        </p>
      )}
    </li>
  );
}

/**
 * Preview SQL of everything the action control would commit: the active tab's staged changes (its
 * structure SQL, or a link to its own sheet) and the pending item operations (sidebar Truncate… /
 * Delete… of tables, routines and sequences), each removable. Commit runs the tab's commit, then the operations.
 */
export function PendingOpsSheet() {
  const connectionId = usePendingSheetStore((s) => s.connectionId);
  const { dialect, name } = useConnectionsStore(
    useShallow((s) => {
      const c = connectionId ? s.byId[connectionId] : undefined;
      return { dialect: c?.info.driver ?? "postgres", name: c?.info.name ?? "" };
    }),
  );
  const ops = usePendingObjectOps(connectionId);
  const active = useActiveTab();
  const tab = active && active.connectionId === connectionId ? active : null;
  const tabCount = useTabPendingCount(tab);
  const qc = useQueryClient();
  const [running, setRunning] = useState(false);
  // Also while a commit started elsewhere (⌘S, the action control) runs.
  const busy = useObjectOpsCommitting(connectionId) || running;
  const open = connectionId !== null && (ops.length > 0 || busy);
  // Emptied (the last operation removed or committed): closed for good, so the next staged
  // operation does not pop it open again.
  useEffect(() => {
    if (connectionId !== null && ops.length === 0 && !busy) closePendingSheet();
  }, [connectionId, ops.length, busy]);

  const commit = async () => {
    if (busy || connectionId === null) return;
    if (tabCount > 0) {
      // The tab's own commit (and its sheet) runs first; the operations follow it.
      closePendingSheet();
      void actions.run("grid.commit");
      return;
    }
    // Nothing staged in the tab: the operations alone, whatever view is up.
    setRunning(true);
    try {
      await runObjectCommit(qc, connectionId);
    } finally {
      setRunning(false);
    }
  };

  return (
    <Sheet open={open} onOpenChange={(o) => !o && !busy && closePendingSheet()}>
      <SheetContent
        side="right"
        className="flex w-[560px] max-w-full flex-col gap-0 p-0 sm:max-w-[560px]"
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            void commit();
          }
        }}
      >
        <SheetHeader className="border-border border-b px-5 py-4">
          <SheetTitle>Preview changes</SheetTitle>
          <SheetDescription>
            {name ? `${name}: ` : ""}
            {ops.length} pending item operation{ops.length === 1 ? "" : "s"}, committed one at a
            time; the first failure stops the rest.
          </SheetDescription>
        </SheetHeader>
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto px-5 py-3">
          {tab && tabCount > 0 && <TabSection tab={tab} count={tabCount} dialect={dialect} />}
          <section aria-label="Item operations" className="flex flex-col gap-1.5">
            <h3 className="text-muted-foreground text-[11px] font-semibold">Item operations</h3>
            <ol className="flex flex-col gap-2">
              {ops.map((o) => (
                <OpItem
                  key={o.id}
                  op={o}
                  dialect={dialect}
                  disabled={busy}
                  onRemove={() => connectionId && !busy && unstageObjectOp(connectionId, o.id)}
                />
              ))}
            </ol>
          </section>
        </div>
        <SheetFooter className="border-border flex-row items-center justify-end gap-2 border-t px-5 py-3">
          <Button variant="ghost" size="sm" onClick={closePendingSheet} disabled={busy}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void commit()} disabled={busy || ops.length === 0}>
            {busy && <Loader2 className="animate-spin" />}
            Commit
            <kbd className="ml-1 text-[10px] opacity-70">{formatCombo("Mod+Enter")}</kbd>
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
