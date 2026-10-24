import { useQueryClient } from "@tanstack/react-query";
import { Dices, Loader2 } from "lucide-react";
import { useId, useMemo, useState, type FormEvent } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { coreDialect } from "@/features/drivers/catalog";
import { useTableMeta } from "@/features/table-tab/useTableRows";
import { tableKey, type Driver } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DUMMY_DATA, dummyInserts, MAX_DUMMY_ROWS, seededRng } from "./builtin/dummyData";
import { PluginCancelled } from "./execute";
import { createHostServices } from "./host";
import { errorText } from "./sandbox/runtime";
import { closeDummyData, usePluginsStore, type DummyDataRequest } from "./store";

/** Characters of SQL the preview shows. */
const PREVIEW_CHARS = 6_000;

function DummyDataForm({ request }: { request: DummyDataRequest }) {
  const qc = useQueryClient();
  const id = useId();
  const table = useMemo(
    () => ({ schema: request.table.schema, name: request.table.name }),
    [request.table.schema, request.table.name],
  );
  const meta = useTableMeta(request.connectionId, table);
  const driver = useConnectionsStore(
    (s) => s.byId[request.connectionId]?.info.driver ?? null,
  ) as Driver | null;
  const [countText, setCountText] = useState("10");
  const [seed, setSeed] = useState(() => Math.floor(Math.random() * 2 ** 31));
  const [busy, setBusy] = useState(false);
  const count = Number(countText);
  const countOk = Number.isInteger(count) && count >= 1 && count <= MAX_DUMMY_ROWS;
  const statements = useMemo(
    () =>
      meta.data && driver && countOk
        ? dummyInserts(coreDialect(driver), meta.data, count, seededRng(seed))
        : [],
    [meta.data, driver, countOk, count, seed],
  );
  const sql = statements.join("\n");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!countOk || busy || sql === "") return;
    setBusy(true);
    const host = createHostServices({
      plugin: { identifier: DUMMY_DATA.identifier, name: DUMMY_DATA.name },
      invocation: {
        source: "sidebar",
        connectionId: request.connectionId,
        driver,
        item: request.table,
        clickedRow: null,
        clickedColumn: null,
        selectedRows: [],
        editorTabId: null,
        sql: null,
      },
    });
    try {
      await host.execute(sql);
      toast.success(`Inserted ${count} ${count === 1 ? "row" : "rows"} into ${table.name}`);
      void qc.invalidateQueries({ queryKey: ["rows", request.connectionId, tableKey(table)] });
      void qc.invalidateQueries({ queryKey: ["count", request.connectionId, tableKey(table)] });
      closeDummyData();
    } catch (err) {
      if (!(err instanceof PluginCancelled))
        toast.error("Could not insert the rows", { description: errorText(err).slice(0, 500) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(e) => void submit(e)} className="flex min-h-0 flex-col gap-3">
      <DialogHeader>
        <DialogTitle className="text-sm">Generate Dummy Data</DialogTitle>
        <DialogDescription className="text-xs">
          Fake rows for <span className="text-foreground font-mono">{table.name}</span>, by column
          type and name. Auto-increment and generated columns keep their defaults.
        </DialogDescription>
      </DialogHeader>
      <div className="flex items-center gap-2 text-xs">
        <label htmlFor={`${id}-count`}>Rows</label>
        <Input
          id={`${id}-count`}
          autoFocus
          inputMode="numeric"
          value={countText}
          onChange={(e) => setCountText(e.target.value.replace(/[^\d]/g, ""))}
          className="h-7 w-24 font-mono text-xs md:text-xs"
          aria-invalid={!countOk}
        />
        <span className="text-muted-foreground">1–{MAX_DUMMY_ROWS.toLocaleString()}</span>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          className="ml-auto"
          onClick={() => setSeed(Math.floor(Math.random() * 2 ** 31))}
        >
          <Dices /> Shuffle
        </Button>
      </div>
      <div className="bg-muted/50 min-h-32 overflow-auto rounded-md border p-2">
        {meta.isPending ? (
          <Loader2 className="text-muted-foreground size-4 animate-spin" aria-label="Loading" />
        ) : meta.isError ? (
          <p className="text-error text-xs">{errorText(meta.error)}</p>
        ) : (
          <pre className="max-h-[40vh] font-mono text-[11px] leading-4 whitespace-pre-wrap">
            {sql.length > PREVIEW_CHARS ? `${sql.slice(0, PREVIEW_CHARS)}\n…` : sql}
          </pre>
        )}
      </div>
      <DialogFooter>
        <Button type="button" variant="ghost" size="sm" onClick={closeDummyData} disabled={busy}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={busy || !countOk || sql === ""}>
          {busy && <Loader2 className="animate-spin" />}
          Insert {countOk ? count : ""} {count === 1 ? "Row" : "Rows"}
        </Button>
      </DialogFooter>
    </form>
  );
}

/** Dummy Data ▸ Generate Dummy Data…: previews the INSERTs, then runs them (Safe Mode applies). */
export function DummyDataDialog() {
  const request = usePluginsStore((s) => s.dummy);
  return (
    <Dialog open={request !== null} onOpenChange={(o) => !o && closeDummyData()}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-xl">
        {request && <DummyDataForm request={request} />}
      </DialogContent>
    </Dialog>
  );
}
