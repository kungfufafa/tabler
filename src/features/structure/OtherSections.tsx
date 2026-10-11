import { Check, ChevronRight, Plus, Trash2, Undo2 } from "lucide-react";
import { Fragment, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import type { Dialect, TableRef, TriggerMeta } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import type { FkRow, IndexRow, RowStatus } from "./model";
import { HEAD_ROW, IconAction, SectionHeader } from "./parts";
import { SqlBlock } from "./SqlBlock";

function rowClass(status: RowStatus) {
  return cn(
    "group border-gridline h-7 border-b",
    status === "added" && "bg-inserted",
    status === "dropped" && "bg-deleted text-muted-foreground line-through",
  );
}

function Th({ children, className }: { children?: ReactNode; className?: string }) {
  return <th className={cn("truncate px-3 font-medium", className)}>{children}</th>;
}

function EmptyRow({ span, text }: { span: number; text: string }) {
  return (
    <tr>
      <td colSpan={span} className="text-muted-foreground px-3 py-2 text-[11px] italic">
        {text}
      </td>
    </tr>
  );
}

function DropToggle({
  status,
  what,
  onDrop,
  onRestore,
  disabled,
}: {
  status: RowStatus;
  what: string;
  onDrop: () => void;
  onRestore: () => void;
  disabled?: boolean;
}) {
  if (disabled) return null;
  return (
    <div
      className={cn(
        "flex justify-end opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
        status === "dropped" && "opacity-100",
      )}
    >
      {status === "dropped" ? (
        <IconAction label={`Restore ${what}`} onClick={onRestore}>
          <Undo2 />
        </IconAction>
      ) : (
        <IconAction
          label={status === "added" ? `Remove new ${what}` : `Drop ${what}`}
          onClick={onDrop}
          className="hover:text-error"
        >
          <Trash2 />
        </IconAction>
      )}
    </div>
  );
}

export function IndexesSection({
  rows,
  editable,
  onAdd,
  onDrop,
  onRestore,
}: {
  rows: IndexRow[];
  editable: boolean;
  onAdd: () => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
}) {
  return (
    <section aria-label="Indexes">
      <SectionHeader title="Indexes" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Index
          </Button>
        )}
      </SectionHeader>
      <table className="w-full min-w-[640px] table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-[26%]" />
          <col className="w-[24%]" />
          <col className="w-16" />
          <col className="w-20" />
          <col />
          <col className="w-12" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>Name</Th>
            <Th>Columns</Th>
            <Th className="text-center">Unique</Th>
            <Th>Method</Th>
            <Th>Condition / definition</Th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const condition = r.predicate ? `WHERE ${r.predicate}` : (r.definition ?? "");
            return (
              <tr key={r.key} className={rowClass(r.status)} data-status={r.status}>
                <td className="truncate px-3 font-mono text-[11px]" title={r.name}>
                  {r.name}
                  {r.primary && (
                    <span className="bg-brand-soft text-brand ml-1.5 rounded px-1 py-px font-sans text-[9px] font-semibold tracking-wide no-underline">
                      PRIMARY
                    </span>
                  )}
                </td>
                <td className="truncate px-3 font-mono text-[11px]" title={r.columns.join(", ")}>
                  {r.columns.join(", ")}
                </td>
                <td className="px-3 text-center">
                  {r.unique && (
                    <Check aria-label="Unique" className="text-muted-foreground mx-auto size-3.5" />
                  )}
                </td>
                <td className="text-muted-foreground truncate px-3 font-mono text-[11px]">
                  {r.method ?? ""}
                </td>
                <td
                  className="text-muted-foreground selectable truncate px-3 font-mono text-[11px]"
                  title={condition}
                >
                  {condition}
                </td>
                <td className="px-1">
                  <DropToggle
                    status={r.status}
                    what="index"
                    disabled={!editable || r.primary}
                    onDrop={() => onDrop(r.key)}
                    onRestore={() => onRestore(r.key)}
                  />
                </td>
              </tr>
            );
          })}
          {rows.length === 0 && <EmptyRow span={6} text="No indexes" />}
        </tbody>
      </table>
    </section>
  );
}

export function ForeignKeysSection({
  rows,
  editable,
  onAdd,
  onDrop,
  onRestore,
  onOpenTable,
}: {
  rows: FkRow[];
  editable: boolean;
  onAdd: () => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onOpenTable: (table: TableRef) => void;
}) {
  return (
    <section aria-label="Foreign keys">
      <SectionHeader title="Foreign keys" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Foreign key
          </Button>
        )}
      </SectionHeader>
      <table className="w-full min-w-[640px] table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-[26%]" />
          <col className="w-[18%]" />
          <col />
          <col className="w-24" />
          <col className="w-24" />
          <col className="w-12" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>Name</Th>
            <Th>Columns</Th>
            <Th>References</Th>
            <Th>On update</Th>
            <Th>On delete</Th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} className={rowClass(r.status)} data-status={r.status}>
              <td className="truncate px-3 font-mono text-[11px]" title={r.name ?? undefined}>
                {r.name ?? <span className="text-muted-foreground italic">auto</span>}
              </td>
              <td className="truncate px-3 font-mono text-[11px]">{r.columns.join(", ")}</td>
              <td className="truncate px-3 font-mono text-[11px]">
                <button
                  type="button"
                  className="text-brand hover:underline"
                  onClick={() => onOpenTable(r.refTable)}
                  title="Open structure"
                >
                  {r.refTable.name}
                </button>
                <span className="text-muted-foreground">({r.refColumns.join(", ")})</span>
              </td>
              <td className="text-muted-foreground truncate px-3 text-[11px]">
                {r.onUpdate ?? "default"}
              </td>
              <td className="text-muted-foreground truncate px-3 text-[11px]">
                {r.onDelete ?? "default"}
              </td>
              <td className="px-1">
                <DropToggle
                  status={r.status}
                  what="foreign key"
                  disabled={!editable || (r.status === "existing" && !r.name)}
                  onDrop={() => onDrop(r.key)}
                  onRestore={() => onRestore(r.key)}
                />
              </td>
            </tr>
          ))}
          {rows.length === 0 && <EmptyRow span={6} text="No foreign keys" />}
        </tbody>
      </table>
    </section>
  );
}

export function TriggersSection({
  triggers,
  dialect,
}: {
  triggers: TriggerMeta[];
  dialect: Dialect;
}) {
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const toggle = (name: string) =>
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  return (
    <section aria-label="Triggers" className="pb-6">
      <SectionHeader title="Triggers" count={triggers.length} />
      <table className="w-full min-w-[640px] table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-[34%]" />
          <col className="w-24" />
          <col />
          <col className="w-20" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>Name</Th>
            <Th>Timing</Th>
            <Th>Events</Th>
            <Th className="text-center">Enabled</Th>
          </tr>
        </thead>
        <tbody>
          {triggers.map((t) => {
            const expanded = open.has(t.name);
            return (
              <Fragment key={t.name}>
                <tr className="border-gridline hover:bg-row-hover h-7 border-b">
                  <td className="px-1.5">
                    <button
                      type="button"
                      aria-expanded={expanded}
                      onClick={() => toggle(t.name)}
                      className="focus-visible:ring-ring/50 flex w-full min-w-0 items-center gap-1 rounded-sm px-1 text-left font-mono text-[11px] outline-none focus-visible:ring-2"
                    >
                      <ChevronRight
                        className={cn(
                          "text-muted-foreground size-3 shrink-0 transition-transform",
                          expanded && "rotate-90",
                        )}
                      />
                      <span className="truncate">{t.name}</span>
                    </button>
                  </td>
                  <td className="text-muted-foreground px-3 text-[11px]">{t.timing ?? ""}</td>
                  <td className="truncate px-3 text-[11px]">{t.events.join(" OR ")}</td>
                  <td className="px-3 text-center">
                    <span
                      aria-label={t.enabled ? "Enabled" : "Disabled"}
                      className={cn(
                        "inline-block size-2 rounded-full",
                        t.enabled ? "bg-success" : "bg-muted-foreground/40",
                      )}
                    />
                  </td>
                </tr>
                {expanded && (
                  <tr className="border-gridline border-b">
                    <td colSpan={4} className="bg-surface-2 px-4 py-2">
                      <SqlBlock sql={t.definition} dialect={dialect} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
          {triggers.length === 0 && <EmptyRow span={4} text="No triggers" />}
        </tbody>
      </table>
    </section>
  );
}
