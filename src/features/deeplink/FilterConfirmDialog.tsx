import { ShieldAlert } from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { answerFilterRequest, useDeepLinkStore } from "./store";

/**
 * A link that opens a table with a raw SQL condition (`condition` / `raw` / `query`, TablePlus
 * b570): the table is open with the condition unchecked in its filter bar, and this asks whether
 * to apply it, showing the table, the connection and the SQL itself. "Apply Filter" checks the
 * row and reloads the table; "Not Now" leaves it unchecked for the user to read. Mounted once
 * per window by `DeepLinkHost`; several links queue up.
 */
export function FilterConfirmDialog() {
  const request = useDeepLinkStore((s) => s.filterRequests[0]);
  if (!request) return null;
  return (
    <AlertDialog
      key={request.id}
      open
      onOpenChange={(open) => {
        // Escape, or a click outside: the same as "Not Now".
        if (!open) answerFilterRequest(request.id, false);
      }}
    >
      <AlertDialogContent className="data-[size=default]:sm:max-w-[34rem]">
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2 text-sm">
            <ShieldAlert className="text-warning size-4 shrink-0" aria-hidden />
            Apply the link&rsquo;s filter?
          </AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex min-w-0 flex-col gap-2 text-xs">
              <p>
                A link opened <span className="text-foreground">{request.table}</span> on{" "}
                <span className="text-foreground">{request.connection}</span> with this SQL
                condition. It runs on the connection as part of the table&rsquo;s query.
              </p>
              <pre
                data-testid="filter-confirm-sql"
                className="bg-surface-2 border-border text-foreground max-h-40 overflow-auto rounded-md border px-2 py-1 font-mono text-xs break-words whitespace-pre-wrap select-text"
              >
                {request.sql}
              </pre>
              <p>&ldquo;Not Now&rdquo; keeps it in the filter bar, unchecked.</p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="sm" onClick={() => answerFilterRequest(request.id, false)}>
            Not Now
          </AlertDialogCancel>
          <AlertDialogAction size="sm" onClick={() => answerFilterRequest(request.id, true)}>
            Apply Filter
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
