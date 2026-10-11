import { Fragment, useMemo } from "react";
import type { Dialect } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { highlightSql } from "./sqlHighlight";

/** Read-only, syntax-highlighted SQL (selectable, wraps long lines). */
export function SqlBlock({
  sql,
  dialect,
  className,
}: {
  sql: string;
  dialect: Dialect;
  className?: string;
}) {
  const lines = useMemo(() => highlightSql(dialect, sql), [dialect, sql]);
  return (
    <pre
      className={cn(
        "selectable font-mono text-[11.5px] leading-[1.5] break-words whitespace-pre-wrap",
        className,
      )}
    >
      {lines.map((segments, i) => (
        <Fragment key={i}>
          {segments.map((s, j) =>
            s.cls ? (
              <span key={j} className={s.cls}>
                {s.text}
              </span>
            ) : (
              <Fragment key={j}>{s.text}</Fragment>
            ),
          )}
          {i < lines.length - 1 && "\n"}
        </Fragment>
      ))}
    </pre>
  );
}
