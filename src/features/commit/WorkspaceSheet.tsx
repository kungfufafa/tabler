import { AlertTriangle, CheckCircle2, Eraser, Loader2, Trash2, X } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
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
import { coreDialect } from "@/features/drivers/catalog";
import { describeOp, unstageObjectOp, type PendingObjectOp } from "@/features/objects/pendingOps";
import { SqlBlock } from "@/features/structure/SqlBlock";
import {
  ipc,
  toIpcError,
  type DefinitionRestore,
  type Dialect,
  type GroupPlan,
  type WorkspacePlan,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { useConnectionsStore } from "@/stores/connections";
import { restoreNote } from "./definitions";
import { useCommitStore, useCommitting, closeWorkspaceSheet, type CommitFailure } from "./store";
import {
  applyWorkspace,
  useWorkspaceSources,
  type WorkspaceProblem,
  type WorkspaceSource,
} from "./workspace";

const NO_PLAN: GroupPlan = { statements: [], warnings: [], transactional: true, error: null };

/** Statements as one script, each ending in `;`. */
function sqlOf(statements: readonly string[]): string {
  return statements.map((s) => (s.trimEnd().endsWith(";") ? s.trimEnd() : `${s};`)).join("\n");
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * Why the changes cannot all run in one transaction on `dialect`: only when `sources` holds a part
 * that is not a data edit (structure, a definition, an item operation) does DDL come into it.
 */
function nonAtomicNote(dialect: Dialect | null, sources: readonly WorkspaceSource[]): string {
  const ddl = sources.some(
    (s) => s.kind === "ddl" || s.kind === "definition" || s.kind === "object",
  );
  const lead = ddl
    ? "Data edits run in one transaction first; the structure changes, definitions and item operations then run one by one, stopping at the first failure."
    : "Data edits run in one transaction.";
  if (dialect === "mysql")
    return ddl
      ? `${lead} MySQL commits every DDL statement on its own: if one fails, the statements before it stay applied.`
      : `${lead} If one fails, all changes are reverted.`;
  if (dialect === "sqlite")
    return `${lead} A table rebuild runs in its own BEGIN … COMMIT block, with foreign-key enforcement switched off around it.`;
  return `${lead} Some statements cannot run inside a transaction block.`;
}

const NO_KEY_WARNING =
  "No primary key: each UPDATE/DELETE matches every original column value and must affect exactly one row.";

/** The statements the commit would run, fetched again whenever the pending work changes. */
function usePlan(connectionId: string | null, sources: readonly WorkspaceSource[]) {
  const [state, setState] = useState<{
    sources: readonly WorkspaceSource[];
    plan: WorkspacePlan | null;
    error: string | null;
  } | null>(null);
  useEffect(() => {
    if (connectionId === null || sources.length === 0) return;
    let cancelled = false;
    ipc.workspace
      .preview(
        connectionId,
        sources.map((s) => s.group),
      )
      .then(
        (plan) => !cancelled && setState({ sources, plan, error: null }),
        (e: unknown) =>
          !cancelled && setState({ sources, plan: null, error: toIpcError(e).message }),
      );
    return () => {
      cancelled = true;
    };
  }, [connectionId, sources]);
  // Loading while the plan on hand is for other sources.
  return state && state.sources === sources ? state : null;
}

function Failure({
  failure,
  atomic,
  dialect,
}: {
  failure: CommitFailure;
  atomic: boolean;
  dialect: Dialect | null;
}) {
  const statement =
    failure.statementIndex !== null ? failure.sql[failure.statementIndex] : undefined;
  return (
    <div
      role="alert"
      className="text-error flex items-start gap-1.5 text-[11px] break-words whitespace-pre-wrap"
    >
      <AlertTriangle className="mt-px size-3.5 shrink-0" />
      <div className="flex min-w-0 flex-col gap-1">
        <span className="font-medium">{failure.message}</span>
        {failure.detail && <span>{failure.detail}</span>}
        {statement && (
          <span className="text-muted-foreground font-mono">
            Statement {(failure.statementIndex ?? 0) + 1}: {statement}
          </span>
        )}
        <span className="text-muted-foreground">
          {atomic
            ? "All changes were reverted."
            : "Changes before it stay committed; the rest stays pending."}
        </span>
        {failure.restore && <RestoreOutcome restore={failure.restore} dialect={dialect} />}
      </div>
    </div>
  );
}

/**
 * MySQL dropped the routine a definition replaces before its new text failed: what became of the
 * old version (put back, or its text when it could not be).
 */
function RestoreOutcome({
  restore,
  dialect,
}: {
  restore: DefinitionRestore;
  dialect: Dialect | null;
}) {
  return (
    <div className="flex flex-col gap-2" data-testid="restore-outcome">
      {restore.restored ? (
        <p
          role="status"
          className="text-foreground flex items-start gap-1.5 text-[11px] break-words whitespace-pre-wrap"
        >
          <CheckCircle2 className="text-success mt-px size-3.5 shrink-0" />
          {restoreNote(restore)}
        </p>
      ) : (
        <>
          <p className="text-error flex items-start gap-1.5 text-[11px] break-words whitespace-pre-wrap">
            <AlertTriangle className="mt-px size-3.5 shrink-0" />
            {restoreNote(restore)}
          </p>
          <div className="border-border bg-surface-2 rounded-md border px-3 py-2">
            <SqlBlock sql={restore.previous} dialect={dialect} />
          </div>
        </>
      )}
    </div>
  );
}

function Section({
  title,
  label,
  count,
  children,
}: {
  title: string;
  label: string;
  count: number;
  children: ReactNode;
}) {
  return (
    <section aria-label={`${label} ${title}`} className="flex flex-col gap-1.5">
      <h3 className="text-muted-foreground flex items-center gap-2 text-[11px] font-semibold">
        <span className="truncate">{title}</span>
        <span className="font-normal">
          {label} · {plural(count, "change")}
        </span>
      </h3>
      <div className="border-border bg-surface-2 flex flex-col gap-2 rounded-md border px-3 py-2">
        {children}
      </div>
    </section>
  );
}

function Statements({
  plan,
  loading,
  dialect,
}: {
  plan: GroupPlan | undefined;
  loading: boolean;
  dialect: Dialect | null;
}) {
  if (loading) return <Loader2 className="text-muted-foreground size-3.5 animate-spin" />;
  if (plan?.error) return <p className="text-error text-xs">{plan.error.message}</p>;
  return (
    <>
      {(plan?.warnings ?? []).map((w) => (
        <p key={w} role="status" className="text-warning text-[11px]">
          {w}
        </p>
      ))}
      <SqlBlock sql={sqlOf(plan?.statements ?? [])} dialect={dialect} />
    </>
  );
}

function OpItem({
  op,
  plan,
  dialect,
  disabled,
  onRemove,
}: {
  op: PendingObjectOp;
  plan: GroupPlan | undefined;
  dialect: Dialect | null;
  disabled: boolean;
  onRemove: () => void;
}) {
  const { verb, what } = describeOp(op.op);
  const Icon = op.kind === "drop" ? Trash2 : Eraser;
  return (
    <li data-pending-op={op.kind} className="flex flex-col gap-1">
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
      <SqlBlock sql={sqlOf(plan?.statements ?? op.statements)} dialect={dialect} />
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

function Problems({ problems }: { problems: readonly WorkspaceProblem[] }) {
  if (problems.length === 0) return null;
  return (
    <div
      role="alert"
      className="border-warning/40 bg-warning/10 text-warning flex flex-col gap-1 rounded-md border px-3 py-2 text-[11px]"
    >
      {problems.map((p) => (
        <p key={`${p.title}:${p.message}`}>
          <span className="font-medium">{p.title}:</span> {p.message}
          {p.detail ? ` ${p.detail}` : ""}
        </p>
      ))}
    </div>
  );
}

const KIND_LABEL: Record<WorkspaceSource["kind"], string> = {
  rows: "Rows",
  results: "Result edits",
  ddl: "Structure",
  definition: "Definition",
  object: "Item operations",
};

/**
 * TablePlus's Preview SQL (⇧⌘P) for the whole workspace: every pending change of the connection
 * grouped by tab — the staged rows of its table tabs, the edits of its query results, the staged
 * structure, the pending sidebar operations — and the one Commit (⌘↩) that runs them all in one
 * transaction (`apply_workspace_changes`).
 */
export function WorkspaceSheet() {
  const sheet = useCommitStore((s) => s.sheet);
  const failure = useCommitStore((s) => s.failure);
  const connectionId = sheet?.connectionId ?? null;
  const { dialect, name } = useConnectionsStore(
    useShallow((s) => {
      const c = connectionId ? s.byId[connectionId] : undefined;
      // Null for a driver beyond the core ones: its SQL shows plain.
      return { dialect: c ? coreDialect(c.info.driver) : "postgres", name: c?.info.name ?? "" };
    }),
  );
  const { sources, problems } = useWorkspaceSources(connectionId);
  const committing = useCommitting(connectionId);
  const planned = usePlan(connectionId, sources);
  const plan = planned?.plan ?? null;
  const loading = planned === null && sources.length > 0;
  const open = sheet !== null;
  const empty = sources.length === 0 && problems.length === 0;
  // Emptied (everything committed, discarded or removed): closed for good.
  useEffect(() => {
    if (open && empty && !committing) closeWorkspaceSheet();
  }, [open, empty, committing]);

  // A modal sheet returns focus to its trigger, and this one has none (⌘S, the action control):
  // put focus back where it was (usually the grid), so ⌘Z / ⌘S / arrows keep working.
  const returnFocus = useRef<HTMLElement | null>(null);

  const atomic =
    plan?.atomic ??
    (dialect !== null &&
      (dialect !== "mysql" || sources.every((s) => s.kind === "rows" || s.kind === "results")));
  const mine = failure && failure.connectionId === connectionId ? failure : null;
  const total = sources.reduce((n, s) => n + s.count, 0);
  const tabs = new Set(sources.map((s) => (s.kind === "object" ? "ops" : s.tabId))).size;
  const objects = sources.flatMap((s, i) => (s.kind === "object" ? [{ s, i }] : []));

  const commit = () => {
    if (committing || connectionId === null || problems.length > 0 || sources.length === 0) return;
    void applyWorkspace(connectionId);
  };

  return (
    <Sheet open={open} onOpenChange={(o) => !o && !committing && closeWorkspaceSheet()}>
      <SheetContent
        side="right"
        className="flex w-[560px] max-w-full flex-col gap-0 p-0 sm:max-w-[560px]"
        onOpenAutoFocus={() => {
          const el = document.activeElement;
          returnFocus.current = el instanceof HTMLElement && el !== document.body ? el : null;
        }}
        onCloseAutoFocus={(e) => {
          const el = returnFocus.current;
          returnFocus.current = null;
          if (!el?.isConnected) return;
          e.preventDefault();
          el.focus({ preventScroll: true });
        }}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
      >
        <SheetHeader className="border-border border-b px-5 py-4">
          <SheetTitle>Preview changes</SheetTitle>
          <SheetDescription>
            {name ? `${name}: ` : ""}
            {plural(total, "pending change")} in {plural(tabs, "tab")}.{" "}
            {atomic
              ? "They run in a single transaction: if one fails, all changes are reverted."
              : nonAtomicNote(dialect, sources)}
          </SheetDescription>
        </SheetHeader>
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto px-5 py-3">
          <Problems problems={problems} />
          {sources.map((s, i) => {
            if (s.kind === "object") return null;
            const groupPlan = plan?.groups[i] ?? NO_PLAN;
            return (
              <Section key={s.key} title={s.title} label={KIND_LABEL[s.kind]} count={s.count}>
                {s.kind === "rows" && s.noKey && (
                  <p className="text-warning text-[11px]">{NO_KEY_WARNING}</p>
                )}
                <Statements plan={groupPlan} loading={loading} dialect={dialect} />
                {mine?.key === s.key && (
                  <Failure failure={mine} atomic={mine.atomic} dialect={dialect} />
                )}
              </Section>
            );
          })}
          {objects.length > 0 && (
            <Section
              title="Item operations"
              label="Sidebar"
              count={objects.reduce((n, { s }) => n + s.count, 0)}
            >
              <ol className="flex flex-col gap-3">
                {objects.map(({ s, i }) =>
                  s.kind === "object" ? (
                    <OpItem
                      key={s.key}
                      op={s.op}
                      plan={plan?.groups[i]}
                      dialect={dialect}
                      disabled={committing}
                      onRemove={() =>
                        connectionId && !committing && unstageObjectOp(connectionId, s.op.id)
                      }
                    />
                  ) : null,
                )}
              </ol>
              {mine && objects.some(({ s }) => s.key === mine.key) && (
                <Failure failure={mine} atomic={mine.atomic} dialect={dialect} />
              )}
            </Section>
          )}
        </div>
        <SheetFooter className="border-border flex-row items-center justify-end gap-2 border-t px-5 py-3">
          <Button variant="ghost" size="sm" onClick={closeWorkspaceSheet} disabled={committing}>
            Cancel
          </Button>
          <Button
            size="sm"
            onClick={commit}
            disabled={committing || sources.length === 0 || problems.length > 0}
          >
            {committing && <Loader2 className="animate-spin" />}
            Commit
            <kbd className="ml-1 text-[10px] opacity-70">{formatCombo("Mod+Enter")}</kbd>
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
