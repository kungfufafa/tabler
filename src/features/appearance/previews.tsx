import { cn } from "@/lib/utils";
import { usePrefs } from "@/stores/prefs";
import type { ThemeBase } from "./builtinThemes";
import { useRowHeightChange } from "./gridMetrics";
import { tokenStyle } from "./previewStyle";
import type { ThemeTokens } from "./tokens";

/** Sample SQL for the editor previews: one of every colour the editor settings list. */
function SqlSample({ showInvisibles }: { showInvisibles?: boolean }) {
  const kw = "text-[color:var(--syn-keyword)]";
  const str = (token: string, text: string) => (
    <span style={{ color: `var(--${token})` }}>{text}</span>
  );
  const dot = showInvisibles ? "·" : " ";
  return (
    <>
      <span className="text-[color:var(--syn-comment)] italic">-- Customers and their orders</span>
      {"\n"}
      <span className={kw}>SELECT</span> id, <span className={kw}>count</span>(*){" "}
      <span className={kw}>AS</span> {str("syn-string-double", '"orders"')},{"\n"}
      {dot}
      {dot}
      {str("syn-string-backtick", "`email`")}, {str("syn-string-bracket", "[region]")}
      {"\n"}
      <span className={kw}>FROM</span> customers <span className={kw}>WHERE</span> name ={" "}
      {str("syn-string", "'Ada'")} <span className={kw}>AND</span> age &gt;{" "}
      <span className="text-[color:var(--syn-number)]">30</span>
      {showInvisibles && <span style={{ color: "var(--syn-invisible)", opacity: 0.45 }}>¬</span>}
      {"\n"}
      <span className={kw}>LIMIT</span> <span className="text-[color:var(--syn-number)]">10</span>;{" "}
      <span
        className="underline decoration-wavy underline-offset-2"
        style={{ color: "var(--syn-error)", textDecorationColor: "var(--syn-error)" }}
      >
        SELEC
      </span>
    </>
  );
}

/** The SQL editor's colours, font and line height as they are right now (reads the live variables). */
export function EditorPreview() {
  const fontSize = usePrefs((p) => p.editorFontSize);
  const invisibles = usePrefs((p) => p.editorShowInvisibles);
  return (
    <pre
      aria-label="SQL editor preview"
      className="border-border bg-surface selectable overflow-x-auto rounded-lg border px-3 py-2.5"
      style={{
        fontSize,
        fontFamily: "var(--editor-font-family)",
        lineHeight: "var(--editor-line-height)",
        color: "var(--syn-other)",
      }}
    >
      <span className="-mx-3 block px-3" style={{ backgroundColor: "var(--editor-statement-bg)" }}>
        <SqlSample showInvisibles={invisibles} />
      </span>
      <span>
        <span className="px-0.5" style={{ backgroundColor: "var(--editor-selection-bg)" }}>
          selected text
        </span>{" "}
        others
      </span>
    </pre>
  );
}

const SAMPLE_ROWS: readonly { id: number; name: string; at: string; state?: string }[] = [
  { id: 1, name: "Ada Lovelace", at: "2024-05-12 09:30:00" },
  { id: 2, name: "Grace Hopper", at: "2024-06-01 14:02:11", state: "modified" },
  { id: 3, name: "Alan Turing", at: "2024-06-18 08:45:27", state: "new" },
  { id: 4, name: "Edsger Dijkstra", at: "2024-07-04 17:20:09", state: "deleted" },
];

/** The data table's font, row height and change-marker colours, drawn like real grid cells. */
export function GridPreview() {
  const rowHeight = useRowHeightChange(() => {});
  const cell =
    "border-gridline absolute top-0 flex h-full items-center overflow-hidden border-r border-b px-2 whitespace-nowrap tabular-nums";
  return (
    <div
      data-scope="grid"
      aria-label="Data table preview"
      role="img"
      className="border-border bg-surface relative overflow-x-auto rounded-lg border"
    >
      <div className="relative" style={{ height: 24, width: 420 }}>
        {[
          ["id", 0, 56],
          ["name", 56, 180],
          ["created_at", 236, 184],
        ].map(([label, left, width]) => (
          <div
            key={label}
            data-header={label}
            className="border-border bg-surface-2 absolute top-0 flex h-full items-center justify-center border-r border-b font-semibold"
            style={{ left: left as number, width: width as number }}
          >
            {label}
          </div>
        ))}
      </div>
      {SAMPLE_ROWS.map((row, i) => (
        <div
          key={row.id}
          className={cn("relative", i % 2 === 1 && "bg-foreground/[2.5%]")}
          style={{ height: rowHeight, width: 420 }}
        >
          <div
            data-cell=""
            className={cn(
              cell,
              "justify-end",
              row.state === "new" && "bg-inserted",
              row.state === "deleted" && "bg-deleted line-through opacity-70",
            )}
            style={{ left: 0, width: 56 }}
          >
            <span className="truncate">{row.id}</span>
          </div>
          <div
            data-cell=""
            className={cn(
              cell,
              row.state === "modified" && "bg-changed",
              row.state === "new" && "bg-inserted",
              row.state === "deleted" && "bg-deleted line-through opacity-70",
            )}
            style={{ left: 56, width: 180 }}
          >
            <span className="truncate">{row.name}</span>
          </div>
          <div
            data-cell=""
            className={cn(
              cell,
              row.state === "new" && "bg-inserted",
              row.state === "deleted" && "bg-deleted line-through opacity-70",
            )}
            style={{ left: 236, width: 184 }}
          >
            <span className="truncate text-(color:--grid-date)">{row.at}</span>
          </div>
        </div>
      ))}
    </div>
  );
}

/** A small window of a theme: the sidebar, a data table with the three change markers and SQL. */
export function ThemePreview({ base, tokens }: { base: ThemeBase; tokens: Partial<ThemeTokens> }) {
  const style = tokenStyle(base, tokens);
  return (
    <div
      aria-label="Theme preview"
      role="img"
      className="border-border text-foreground flex overflow-hidden rounded-lg border text-xs"
      style={{
        ...style,
        backgroundColor: "var(--background)",
        color: "var(--foreground)",
        colorScheme: base,
      }}
    >
      <div
        className="border-border flex w-24 shrink-0 flex-col gap-1 border-r p-2"
        style={{ backgroundColor: "var(--sidebar)", borderColor: "var(--border)" }}
      >
        <span style={{ color: "var(--muted-foreground)" }}>Tables</span>
        <span>customers</span>
        <span className="rounded px-1" style={{ backgroundColor: "var(--accent)" }}>
          orders
        </span>
        <span style={{ color: "var(--brand)" }}>invoices</span>
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <div
          className="grid grid-cols-[2.5rem_1fr_1fr] border-b"
          style={{ backgroundColor: "var(--surface-2)", borderColor: "var(--border)" }}
        >
          <span className="px-2 py-1 font-semibold">id</span>
          <span className="px-2 py-1 font-semibold">name</span>
          <span className="px-2 py-1 font-semibold">created</span>
        </div>
        {(
          [
            ["1", "Ada", "2024-05-12", null],
            ["2", "Grace", "2024-06-01", "var(--change-modified)"],
            ["3", "Alan", "2024-06-18", "var(--change-new)"],
            ["4", "Edsger", "2024-07-04", "var(--change-deleted)"],
          ] as const
        ).map(([id, name, date, marker]) => (
          <div
            key={id}
            className="grid grid-cols-[2.5rem_1fr_1fr] border-b"
            style={{
              backgroundColor: "var(--surface)",
              borderColor: "var(--gridline)",
            }}
          >
            <span className="px-2 py-1 text-right" style={{ backgroundColor: marker ?? undefined }}>
              {id}
            </span>
            <span className="px-2 py-1" style={{ backgroundColor: marker ?? undefined }}>
              {name}
            </span>
            <span
              className="px-2 py-1"
              style={{ backgroundColor: marker ?? undefined, color: "var(--grid-date)" }}
            >
              {date}
            </span>
          </div>
        ))}
        <pre
          className="px-2 py-1.5 font-mono leading-[1.5]"
          style={{ backgroundColor: "var(--surface)", color: "var(--syn-other)" }}
        >
          <SqlSample />
        </pre>
      </div>
    </div>
  );
}
