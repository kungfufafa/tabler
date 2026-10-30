import { useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, SquareTerminal } from "lucide-react";
import { useState, type ReactNode } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Skeleton } from "@/components/ui/skeleton";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { BarIconButton, BottomBarFrame } from "@/features/table-tab/BarParts";
import { ViewSwitch } from "@/features/tabs/ViewSwitch";
import { has, nounsOf, sessionDialect } from "@/lib/caps";
import { errorWithHint } from "@/lib/errorText";
import { ipc, type SessionCaps, type TableRef } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ShowDdlDialog } from "./ShowDdlDialog";
import { tableOptionRows } from "./tableOptions";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="flex flex-col gap-1">
      <h3 className="text-muted-foreground text-[11px] font-medium tracking-wide uppercase">
        {title}
      </h3>
      {children}
    </section>
  );
}

function Table({ head, rows }: { head: string[]; rows: Array<Array<ReactNode>> }) {
  return (
    <table className="border-border w-full border-collapse border text-xs">
      <thead>
        <tr className="bg-surface-2">
          {head.map((h) => (
            <th key={h} className="border-border border px-2 py-1 text-left font-medium">
              {h}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((cells, i) => (
          <tr key={i}>
            {cells.map((c, j) => (
              <td key={j} className={cn("border-border border px-2 py-1", j === 0 && "font-mono")}>
                {c}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * The Structure tab of a session that cannot edit its structure the core way (no
 * `Feature::StructureEdit`, or SQL the core generators do not write): the columns, indexes,
 * foreign keys and options of the table as the server describes them, read-only, with Show DDL
 * where the session offers it. Its levels are named as the session names them ("Collection").
 */
export function ReadOnlyStructure({
  tabId,
  connectionId,
  table,
  caps,
}: {
  tabId: string;
  connectionId: string;
  table: TableRef;
  caps: SessionCaps;
}) {
  const qc = useQueryClient();
  const key = schemaKeys.table(connectionId, table.schema, table.name);
  const meta = useQuery({
    queryKey: key,
    queryFn: () => ipc.schema.table(connectionId, table),
    staleTime: 5 * 60_000,
  });
  const [showDdl, setShowDdl] = useState(false);
  const noun = nounsOf(caps).table;
  const showsDdl = has(caps, "tableDdl");
  const m = meta.data;
  const options = m ? tableOptionRows(m.options, sessionDialect(caps) === "mysql") : [];
  return (
    <div className="flex h-full min-h-0 flex-col" data-slot="read-only-structure">
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {meta.isPending ? (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-5 w-48" />
            <Skeleton className="h-40 w-full" />
          </div>
        ) : meta.isError ? (
          <EmptyState
            title={`Could not load the ${noun.toLowerCase()}`}
            description={errorWithHint(meta.error)}
          />
        ) : m ? (
          <div className="flex flex-col gap-4">
            <p className="text-muted-foreground text-xs">
              <span className="text-foreground font-mono">
                {table.schema ? `${table.schema}.` : ""}
                {table.name}
              </span>
              {` · ${noun} (read-only)`}
              {m.comment ? ` · ${m.comment}` : ""}
            </p>
            <Section title="Columns">
              <Table
                head={["Name", "Type", "Nullable", "Default", "Key", "Comment"]}
                rows={m.columns.map((c) => [
                  c.name,
                  c.dataType,
                  c.nullable ? "YES" : "NO",
                  c.defaultExpr ?? "",
                  c.isPk ? "PRIMARY" : "",
                  c.comment ?? "",
                ])}
              />
            </Section>
            {m.indexes.length > 0 && (
              <Section title="Indexes">
                <Table
                  head={["Name", "Columns", "Unique"]}
                  rows={m.indexes.map((i) => [i.name, i.columns.join(", "), i.unique ? "YES" : ""])}
                />
              </Section>
            )}
            {m.foreignKeys.length > 0 && (
              <Section title="Foreign keys">
                <Table
                  head={["Name", "Columns", "References"]}
                  rows={m.foreignKeys.map((f) => [
                    f.name,
                    f.columns.join(", "),
                    `${f.refTable.name}(${f.refColumns.join(", ")})`,
                  ])}
                />
              </Section>
            )}
            {options.length > 0 && (
              <Section title="Options">
                <Table head={["Option", "Value"]} rows={options} />
              </Section>
            )}
          </div>
        ) : null}
      </div>
      <BottomBarFrame aria-label="Table structure">
        <ViewSwitch tabId={tabId} view="structure" />
        <span className="flex-1" />
        {showsDdl && (
          <BarIconButton
            label="Show DDL"
            tooltip="The statement that creates it, as the server gives it"
            onClick={() => setShowDdl(true)}
          >
            <SquareTerminal />
          </BarIconButton>
        )}
        <BarIconButton
          label="Refresh"
          onClick={() => {
            void ipc.schema.invalidate(connectionId, table).catch(() => {});
            void qc.invalidateQueries({ queryKey: key });
          }}
          disabled={meta.isFetching}
        >
          <RefreshCw className={cn(meta.isFetching && "animate-spin")} />
        </BarIconButton>
      </BottomBarFrame>
      {showsDdl && (
        <ShowDdlDialog
          open={showDdl}
          onOpenChange={setShowDdl}
          connectionId={connectionId}
          table={table}
          dialect={sessionDialect(caps)}
        />
      )}
    </div>
  );
}
