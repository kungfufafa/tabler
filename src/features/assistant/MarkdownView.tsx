import { Check, Copy, CornerDownLeft, Play } from "lucide-react";
import { Fragment, memo, useMemo, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { highlightSql } from "@/features/structure/sqlHighlight";
import { isTauri, type Dialect } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { copyText } from "./actions";
import { isSqlCode, parseInline, parseMarkdown, type Block, type Inline } from "./markdown";

/** What SQL code blocks can do (absent: Copy only). */
export interface SqlActions {
  insert: (sql: string) => void;
  run: (sql: string) => void;
}

interface Props {
  text: string;
  /** SQL highlighting dialect. */
  dialect: Dialect;
  /** The reply is still arriving: actions wait for a block to be complete. */
  streaming?: boolean;
  sql?: SqlActions | null;
}

async function openLink(href: string): Promise<void> {
  if (!/^https?:\/\//.test(href)) return;
  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(href);
  } else {
    window.open(href, "_blank", "noopener,noreferrer");
  }
}

function renderInline(nodes: Inline[], key = "i"): ReactNode[] {
  return nodes.map((n, i) => {
    const k = `${key}.${i}`;
    switch (n.t) {
      case "text":
        return <Fragment key={k}>{n.text}</Fragment>;
      case "code":
        return (
          <code
            key={k}
            className="bg-surface-2 border-border rounded-[4px] border px-1 py-px font-mono text-[0.92em] break-words"
          >
            {n.text}
          </code>
        );
      case "strong":
        return (
          <strong key={k} className="font-semibold">
            {renderInline(n.children, k)}
          </strong>
        );
      case "em":
        return <em key={k}>{renderInline(n.children, k)}</em>;
      case "link":
        return (
          <a
            key={k}
            href={n.href}
            title={n.href}
            className="text-primary underline underline-offset-2"
            onClick={(e) => {
              e.preventDefault();
              void openLink(n.href);
            }}
          >
            {renderInline(n.children, k)}
          </a>
        );
    }
  });
}

const inline = (text: string, key: string) => renderInline(parseInline(text), key);

function CodeBlock({
  lang,
  text,
  complete,
  dialect,
  sql,
}: {
  lang: string;
  text: string;
  complete: boolean;
  dialect: Dialect;
  sql?: SqlActions | null;
}) {
  const isSql = isSqlCode(lang, text);
  const [copied, setCopied] = useState(false);
  const lines = useMemo(
    () =>
      isSql ? highlightSql(dialect, text) : text.split("\n").map((t) => [{ text: t, cls: "" }]),
    [isSql, dialect, text],
  );
  return (
    <div className="border-border bg-surface overflow-hidden rounded-md border" data-code-block>
      <div className="border-border bg-surface-2 dark:bg-background/40 flex h-6 items-center gap-0.5 border-b pr-0.5 pl-2">
        <span className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-[10px]">
          {lang || (isSql ? "sql" : "code")}
        </span>
        {isSql && sql && (
          <>
            <Button
              variant="ghost"
              size="xs"
              className="h-5 px-1.5 text-[11px]"
              disabled={!complete}
              onClick={() => sql.insert(text)}
              title="Insert into the SQL editor"
            >
              <CornerDownLeft />
              Insert
            </Button>
            <Button
              variant="ghost"
              size="xs"
              className="h-5 px-1.5 text-[11px]"
              disabled={!complete}
              onClick={() => sql.run(text)}
              title="Run in a new query tab (safe mode applies)"
            >
              <Play />
              Run
            </Button>
          </>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          className="size-5"
          aria-label="Copy code"
          title="Copy"
          onClick={() => {
            copyText(text, isSql ? "SQL" : "code");
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
        >
          {copied ? <Check /> : <Copy />}
        </Button>
      </div>
      <pre className="selectable overflow-x-auto px-2 py-1.5 font-mono text-[11.5px] leading-[1.5]">
        <code>
          {lines.map((segments, i) => (
            <Fragment key={i}>
              {i > 0 && "\n"}
              {segments.map((s, j) =>
                s.cls ? (
                  <span key={j} className={s.cls}>
                    {s.text}
                  </span>
                ) : (
                  <Fragment key={j}>{s.text}</Fragment>
                ),
              )}
            </Fragment>
          ))}
        </code>
      </pre>
    </div>
  );
}

function Blocks({
  blocks,
  path,
  ...rest
}: { blocks: Block[]; path: string } & Omit<Props, "text">) {
  return (
    <>
      {blocks.map((b, i) => {
        const key = `${path}.${i}`;
        switch (b.t) {
          case "p":
            return (
              <p key={key} className="whitespace-pre-wrap">
                {inline(b.text, key)}
              </p>
            );
          case "h":
            return (
              <p key={key} role="heading" aria-level={b.level} className="font-semibold">
                {inline(b.text, key)}
              </p>
            );
          case "code":
            return (
              <CodeBlock
                key={key}
                lang={b.lang}
                text={b.text}
                complete={b.closed || !rest.streaming}
                dialect={rest.dialect}
                sql={rest.sql}
              />
            );
          case "list": {
            const Tag = b.ordered ? "ol" : "ul";
            return (
              <Tag
                key={key}
                start={b.ordered && b.start !== 1 ? b.start : undefined}
                className={cn("flex flex-col gap-1 pl-4", b.ordered ? "list-decimal" : "list-disc")}
              >
                {b.items.map((item, j) => (
                  <li key={j} className="flex-col gap-1.5 pl-0.5 [&>*+*]:mt-1.5">
                    <Blocks blocks={item} path={`${key}.${j}`} {...rest} />
                  </li>
                ))}
              </Tag>
            );
          }
          case "quote":
            return (
              <blockquote
                key={key}
                className="border-border text-muted-foreground flex flex-col gap-1.5 border-l-2 pl-2"
              >
                <Blocks blocks={b.blocks} path={key} {...rest} />
              </blockquote>
            );
          case "table":
            return (
              <div key={key} className="border-border overflow-x-auto rounded-md border">
                <table className="w-full border-collapse text-[11.5px]">
                  <thead className="bg-surface-2 dark:bg-background/40">
                    <tr>
                      {b.header.map((h, j) => (
                        <th
                          key={j}
                          style={{ textAlign: b.align[j] ?? "left" }}
                          className="border-border border-b px-1.5 py-0.5 font-medium"
                        >
                          {inline(h, `${key}.h${j}`)}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {b.rows.map((row, r) => (
                      <tr key={r} className="border-border border-b last:border-b-0">
                        {b.header.map((_, j) => (
                          <td
                            key={j}
                            style={{ textAlign: b.align[j] ?? "left" }}
                            className="px-1.5 py-0.5 align-top"
                          >
                            {inline(row[j] ?? "", `${key}.${r}.${j}`)}
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          case "hr":
            return <hr key={key} className="border-border" />;
        }
      })}
    </>
  );
}

/** An assistant reply rendered from its Markdown (no HTML is injected). */
export const Markdown = memo(function Markdown({ text, ...rest }: Props) {
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  return (
    <div className="selectable flex min-w-0 flex-col gap-2 break-words">
      <Blocks blocks={blocks} path="b" {...rest} />
    </div>
  );
});
