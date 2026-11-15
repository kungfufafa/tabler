import { FileCode2, Plus, Settings2, X, Zap } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { SectionLabel } from "@/components/SectionLabel";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Toggle } from "@/components/ui/toggle";
import type { CharsetInfo, Dialect, TableOptions } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { ChoiceCell } from "./cells";
import type { TableOptionField } from "./objects";
import type { StructureView } from "./model";
import { BAR_BUTTON, CellInput } from "./parts";
import { tableOptionRows } from "./tableOptions";

function Label({ children }: { children: ReactNode }) {
  return (
    <span aria-hidden className="text-muted-foreground shrink-0 text-xs font-medium select-none">
      {children}
    </span>
  );
}

/**
 * The structure header (TablePlus): Name (edit it to rename the table), Primary (the primary key
 * as tokens: remove one, add one), the table comment, MySQL table options, and the Triggers and
 * Info buttons.
 */
export function StructureHeader({
  view,
  dialect,
  editable,
  showTriggers,
  triggerCount,
  charsets,
  engines,
  onRename,
  onSetPrimaryKey,
  onSetComment,
  onSetOption,
  onToggleTriggers,
  onInfo,
}: {
  view: StructureView;
  dialect: Dialect;
  editable: boolean;
  showTriggers: boolean;
  triggerCount: number;
  charsets: readonly CharsetInfo[];
  engines: readonly string[];
  onRename: (name: string) => void;
  onSetPrimaryKey: (columns: string[]) => void;
  onSetComment: (comment: string) => void;
  onSetOption: (field: TableOptionField, value: string | number | null) => void;
  onToggleTriggers: () => void;
  onInfo: () => void;
}) {
  const info = view.info;
  const pk = view.primaryKey;
  const candidates = view.columns
    .filter((c) => c.status !== "dropped" && !pk.includes(c.name))
    .map((c) => c.name);
  return (
    <div
      role="group"
      aria-label="Table"
      className="border-border bg-surface-2 flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1.5 border-b px-3 py-1.5"
    >
      <div className="flex min-w-0 items-center gap-1.5">
        <Label>Name</Label>
        <div
          className={cn(
            "border-border w-44 rounded-md border",
            info.opAt.name !== undefined && "bg-changed",
          )}
        >
          <CellInput
            value={info.name}
            onCommit={onRename}
            cell="0|tableName"
            cellKey="table:name"
            label="Table name"
            readOnly={!editable}
            className="font-mono font-medium"
          />
        </div>
      </div>
      <div className="flex min-w-0 items-center gap-1.5">
        <Label>Primary</Label>
        <div
          role="list"
          aria-label="Primary key"
          className="border-border flex min-h-6 min-w-24 flex-wrap items-center gap-1 rounded-md border px-1 py-0.5"
        >
          {pk.map((c) => (
            <span
              role="listitem"
              key={c}
              className="bg-brand-soft text-foreground inline-flex h-5 items-center gap-0.5 rounded-sm px-1.5 font-mono text-xs"
            >
              {c}
              {editable && (
                <button
                  type="button"
                  aria-label={`Remove ${c} from the primary key`}
                  onClick={() => onSetPrimaryKey(pk.filter((x) => x !== c))}
                  className="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 -mr-0.5 rounded-sm transition-colors outline-none focus-visible:ring-2"
                >
                  <X className="size-3" />
                </button>
              )}
            </span>
          ))}
          {pk.length === 0 && !editable && <span className="text-null text-xs italic">none</span>}
          {editable && candidates.length > 0 && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label="Add a column to the primary key"
                  className="text-muted-foreground hover:text-foreground hover:bg-accent aria-expanded:bg-accent focus-visible:ring-ring/50 flex size-5 items-center justify-center rounded-sm transition-colors outline-none focus-visible:ring-2"
                >
                  <Plus className="size-3" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent className="max-h-72 min-w-36 overflow-auto">
                {candidates.map((c) => (
                  <DropdownMenuItem
                    key={c}
                    className="font-mono"
                    onSelect={() => onSetPrimaryKey([...pk, c])}
                  >
                    {c}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>
      {dialect !== "sqlite" && (
        <div className="flex min-w-0 flex-1 items-center gap-1.5">
          <Label>Comment</Label>
          <div
            className={cn(
              "border-border min-w-32 flex-1 rounded-md border",
              info.opAt.comment !== undefined && "bg-changed",
            )}
          >
            <CellInput
              value={info.comment ?? ""}
              onCommit={onSetComment}
              cell="0|tableComment"
              cellKey="table:comment"
              label="Table comment"
              readOnly={!editable}
              placeholder={editable ? "No comment" : undefined}
            />
          </div>
        </div>
      )}
      <span className={cn(dialect === "sqlite" && "flex-1")} />
      <div className="flex items-center gap-1">
        {dialect !== "mysql" && <StorageOptions options={info.options} />}
        {dialect === "mysql" && (
          <OptionsButton
            options={info.options}
            changed={info.opAt.options !== undefined}
            editable={editable}
            charsets={charsets}
            engines={engines}
            onSet={onSetOption}
          />
        )}
        <Toggle
          size="sm"
          pressed={showTriggers}
          onPressedChange={onToggleTriggers}
          aria-label="Triggers"
          className="hover:bg-accent aria-pressed:bg-brand-soft aria-pressed:text-brand data-[state=on]:bg-brand-soft data-[state=on]:text-brand h-6 gap-1 px-2 text-xs"
        >
          <Zap />
          Triggers
          {triggerCount > 0 && (
            <span className="text-muted-foreground tabular-nums">{triggerCount}</span>
          )}
        </Toggle>
        <Button
          variant="ghost"
          size="xs"
          onClick={onInfo}
          aria-label="Info"
          title="The CREATE statement"
          className={BAR_BUTTON}
        >
          <FileCode2 /> Info
        </Button>
      </div>
    </div>
  );
}

const ROW_FORMATS = ["DEFAULT", "DYNAMIC", "COMPACT", "REDUNDANT", "COMPRESSED"];

/**
 * The options a PostgreSQL flavor that distributes its tables reports (Greenplum, Redshift):
 * read-only; nothing for PostgreSQL itself and SQLite, which report none.
 */
function StorageOptions({ options }: { options: TableOptions | null }) {
  const rows = tableOptionRows(options, false);
  if (rows.length === 0) return null;
  return (
    <span data-testid="table-storage" className="text-muted-foreground px-1 text-xs">
      {rows.map(([label, value]) => `${label}: ${value}`).join(" · ")}
    </span>
  );
}

/** MySQL Engine, Charset, Collation, Auto increment, Row format. */
function OptionsButton({
  options,
  changed,
  editable,
  charsets,
  engines,
  onSet,
}: {
  options: TableOptions | null;
  changed: boolean;
  editable: boolean;
  charsets: readonly CharsetInfo[];
  engines: readonly string[];
  onSet: (field: TableOptionField, value: string | number | null) => void;
}) {
  const aiId = useId();
  const [open, setOpen] = useState(false);
  const [ai, setAi] = useState("");
  const o = options;
  const charset = charsets.find((c) => c.name === o?.charset);
  const collation = o?.collation ?? charset?.defaultCollation ?? null;
  const list = (values: readonly string[], current: string | null | undefined) => {
    const all = [...values];
    if (current && !all.some((v) => v.toLowerCase() === current.toLowerCase()))
      all.unshift(current);
    return all.map((v) => ({ value: v, label: v }));
  };
  const rowFormat = o?.rowFormat?.toUpperCase() ?? null;
  return (
    <Popover
      open={open}
      onOpenChange={(v) => {
        if (v) setAi(o?.autoIncrement != null ? String(o.autoIncrement) : "");
        setOpen(v);
      }}
    >
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Table options"
          className={cn(BAR_BUTTON, changed && "bg-changed")}
        >
          <Settings2 /> Options
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 gap-2 p-3 text-xs">
        <SectionLabel>Table options</SectionLabel>
        <div className="grid grid-cols-[5.5rem_minmax(0,1fr)] items-center gap-x-3 gap-y-1.5">
          <span className="text-muted-foreground text-xs">Engine</span>
          <ChoiceCell
            value={o?.engine ?? null}
            options={list(engines, o?.engine)}
            onChange={(v) => onSet("engine", v)}
            label="Engine"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono"
          />
          <span className="text-muted-foreground text-xs">Charset</span>
          <ChoiceCell
            value={o?.charset ?? null}
            options={list(
              charsets.map((c) => c.name),
              o?.charset,
            )}
            onChange={(v) => onSet("charset", v)}
            label="Charset"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono"
          />
          <span className="text-muted-foreground text-xs">Collation</span>
          <ChoiceCell
            value={collation}
            options={list(charset?.collations ?? [], collation)}
            onChange={(v) => onSet("collation", v)}
            label="Collation"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono"
          />
          <label htmlFor={aiId} className="text-muted-foreground text-xs">
            Auto increment
          </label>
          <Input
            id={aiId}
            value={ai}
            inputMode="numeric"
            readOnly={!editable}
            onChange={(e) => setAi(e.target.value.replace(/[^\d]/g, ""))}
            onBlur={() => {
              if (ai !== (o?.autoIncrement != null ? String(o.autoIncrement) : ""))
                onSet("autoIncrement", ai === "" ? null : Number(ai));
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            }}
            placeholder="Next value"
            className="h-6 rounded-md font-mono text-xs md:text-xs"
          />
          <span className="text-muted-foreground text-xs">Row format</span>
          <ChoiceCell
            value={rowFormat}
            options={list(ROW_FORMATS, rowFormat)}
            onChange={(v) => onSet("rowFormat", v)}
            label="Row format"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono"
          />
        </div>
        <p className="text-muted-foreground text-xs leading-4">
          A new charset or collation converts every text column (CONVERT TO CHARACTER SET).
        </p>
      </PopoverContent>
    </Popover>
  );
}
