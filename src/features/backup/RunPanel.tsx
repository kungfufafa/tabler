import { CircleAlert, CircleCheck, Loader2, TriangleAlert } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { isErrorLine } from "./logLines";
import { basename, formatElapsed } from "./options";
import { bytesLabel, type BackupRun } from "./store";

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/**
 * The running (or just ended) job: what runs, the binary's output as a log, elapsed time and
 * bytes. The binaries report no progress, so there is no progress bar.
 */
export function RunPanel({ run }: { run: BackupRun }) {
  const running = run.status === "running";
  const now = useNow(running);
  const log = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useLayoutEffect(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [run.log.length]);

  const title =
    run.mode === "backup"
      ? `${running ? "Backing up" : "Backup of"} ${run.database}`
      : `${running ? "Restoring" : "Restore of"} ${run.path ? basename(run.path) : "the file"} into ${run.database}`;
  const elapsed = formatElapsed((run.endedAt ?? now) - run.startedAt);
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2 px-4 py-3">
      <div className="flex min-w-0 items-center gap-2 text-sm font-medium">
        {running ? (
          <Loader2 className="text-brand size-4 shrink-0 animate-spin" />
        ) : run.status === "failed" ? (
          <CircleAlert className="text-error size-4 shrink-0" />
        ) : run.errors > 0 ? (
          <TriangleAlert className="text-warning size-4 shrink-0" />
        ) : (
          <CircleCheck className="text-success size-4 shrink-0" />
        )}
        <span className="truncate" title={title}>
          {title}
        </span>
        <span className="text-muted-foreground shrink-0 text-xs font-normal">
          · {run.profileName}
        </span>
      </div>
      {run.path && (
        <div className="text-muted-foreground truncate font-mono text-[11px]" title={run.path}>
          {run.mode === "backup" ? "→ " : "← "}
          {run.path}
        </div>
      )}
      <div
        ref={log}
        role="log"
        aria-label="Output"
        aria-live="polite"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="border-border bg-surface-2 min-h-0 flex-1 overflow-auto rounded-md border px-2 py-1.5 font-mono text-[11px] leading-4 whitespace-pre-wrap"
      >
        {run.command && <div className="text-muted-foreground break-all">$ {run.command}</div>}
        {run.log.map((line, i) => (
          <div
            key={i}
            className={cn(
              "break-all",
              line.stream === "info" && "text-muted-foreground italic",
              line.stream !== "info" && isErrorLine(line.text) && "text-error",
            )}
          >
            {line.text}
          </div>
        ))}
        {run.log.length === 0 && running && (
          <div className="text-muted-foreground">Waiting for output…</div>
        )}
      </div>
      {run.status === "failed" && run.error && (
        <div role="alert" className="text-error flex items-start gap-1.5 text-xs">
          <CircleAlert className="mt-px size-3.5 shrink-0" />
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="line-clamp-3 break-words" title={run.error}>
              {run.error}
            </span>
            {run.errorHint && <span className="text-muted-foreground">{run.errorHint}</span>}
          </span>
        </div>
      )}
      {run.status === "done" && run.errors > 0 && (
        <div role="alert" className="text-warning flex items-start gap-1.5 text-xs">
          <TriangleAlert className="mt-px size-3.5 shrink-0" />
          <span>
            The restore finished, but {run.errors} statement{run.errors === 1 ? "" : "s"} failed
            (see the log). Add <span className="font-mono">-v ON_ERROR_STOP=1</span> to stop at the
            first error.
          </span>
        </div>
      )}
      <div
        role="status"
        className="text-muted-foreground flex items-center gap-2 text-xs tabular-nums"
      >
        <span>{elapsed}</span>
        <span>·</span>
        <span>{bytesLabel(run)}</span>
        {run.cancelling && <span>· cancelling…</span>}
      </div>
    </div>
  );
}
