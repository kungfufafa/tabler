import { useQuery } from "@tanstack/react-query";
import { KeyRound } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { ValueKindIcon } from "@/components/TypeIcon";
import { Skeleton } from "@/components/ui/skeleton";
import { ipc } from "@/lib/ipc";
import type { Tab } from "@/stores/tabs";

/** M1 placeholder: proves the metadata round-trip. The virtualized data grid replaces this in M2. */
export function TableTab({ tab }: { tab: Extract<Tab, { kind: "table" }> }) {
  const meta = useQuery({
    queryKey: ["table", tab.connectionId, tab.table.schema ?? "", tab.table.name],
    queryFn: () => ipc.schema.table(tab.connectionId, tab.table),
  });
  if (meta.isPending) {
    return (
      <div className="flex flex-col gap-2 p-4">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-4 w-96" />
      </div>
    );
  }
  if (meta.isError)
    return <EmptyState title="Could not load table" description={meta.error.message} />;
  const m = meta.data;
  return (
    <div className="flex h-full flex-col overflow-auto p-4 text-sm" data-scope="grid">
      <h2 className="font-mono text-base font-semibold">
        {m.table.schema ? <span className="text-muted-foreground">{m.table.schema}.</span> : null}
        {m.table.name}
      </h2>
      <p className="text-muted-foreground mt-1 text-xs">
        {m.columns.length} columns · primary key{" "}
        {m.primaryKey.length ? m.primaryKey.join(", ") : "none"} · data grid arrives in M2
      </p>
      <ul className="mt-3 flex flex-col gap-0.5 font-mono text-xs">
        {m.columns.map((c) => (
          <li key={c.name} className="flex items-center gap-2">
            {c.isPk ? (
              <KeyRound className="text-warning size-3" />
            ) : (
              <ValueKindIcon kind={c.kind} />
            )}
            <span className="w-40 truncate">{c.name}</span>
            <span className="text-muted-foreground">{c.dataType}</span>
            {!c.nullable && <span className="text-muted-foreground text-[10px]">not null</span>}
            {c.defaultExpr && (
              <span className="text-muted-foreground text-[10px]">= {c.defaultExpr}</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
