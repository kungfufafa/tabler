import { useQuery } from "@tanstack/react-query";
import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { ipc, type Dialect, type TableRef } from "@/lib/ipc";
import { ddlKey } from "./invalidate";
import { SqlBlock } from "./SqlBlock";

/** Read-only CREATE statement of a table / view with a copy button. */
export function ShowDdlDialog({
  open,
  onOpenChange,
  connectionId,
  table,
  dialect,
  returnFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  table: TableRef;
  dialect: Dialect;
  returnFocus?: () => void;
}) {
  const ddl = useQuery({
    queryKey: ddlKey(connectionId, table),
    queryFn: () => ipc.ddl.tableDdl(connectionId, table),
    enabled: open,
    staleTime: 0,
  });
  const copy = () => {
    if (!ddl.data) return;
    void navigator.clipboard?.writeText(ddl.data).then(
      () => toast.success("DDL copied"),
      () => toast.error("Could not copy"),
    );
  };
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="flex max-h-[80vh] flex-col gap-0 p-0 sm:max-w-3xl"
        onCloseAutoFocus={(e) => {
          if (!returnFocus) return;
          e.preventDefault();
          returnFocus();
        }}
      >
        <DialogHeader className="border-border border-b px-4 py-3">
          <DialogTitle className="font-mono text-sm">
            {table.schema ? `${table.schema}.` : ""}
            {table.name}
          </DialogTitle>
          <DialogDescription className="text-xs">
            CREATE statement, as the database describes it now.
          </DialogDescription>
        </DialogHeader>
        <div className="bg-surface-2 min-h-24 flex-1 overflow-auto px-4 py-3">
          {ddl.isPending ? (
            <div className="flex flex-col gap-1.5">
              <Skeleton className="h-3.5 w-1/2" />
              <Skeleton className="h-3.5 w-2/3" />
              <Skeleton className="h-3.5 w-1/3" />
            </div>
          ) : ddl.isError ? (
            <p className="text-error text-xs">{ddl.error.message}</p>
          ) : (
            <SqlBlock sql={ddl.data} dialect={dialect} />
          )}
        </div>
        <DialogFooter className="m-0 rounded-b-xl px-4 py-2.5">
          <Button variant="outline" size="sm" onClick={copy} disabled={!ddl.data}>
            <Copy /> Copy
          </Button>
          <DialogClose asChild>
            <Button size="sm">Done</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
