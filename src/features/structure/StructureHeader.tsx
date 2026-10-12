import { FileCode2, Plus, Settings2, X, Zap } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
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
import { CellInput } from "./parts";

function Label({ children }: { children: ReactNode }) {
  return (
    <span
      aria-hidden
      className="text-muted-foreground shrink-0 text-[11px] font-medium select-none"
    >
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
      className="border-border bg-surface-2/60 flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1.5 border-b px-3 py-1.5"
    >
      <div className="flex min-w-0 items-center gap-1.5">
        <Label>Name</Label>
        <div
          className={cn(
            "border-border w-44 rounded-sm border",
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
          className="border-border flex min-h-6 min-w-24 flex-wrap items-center gap-1 rounded-sm border px-1 py-0.5"
        >
          {pk.map((c) => (
            <span
              role="listitem"
              key={c}
              className="bg-brand-soft text-foreground inline-flex h-5 items-center gap-0.5 rounded px-1.5 font-mono text-[11px]"
            >
              {c}
              {editable && (
                <button
                  type="button"
                  aria-label={`Remove ${c} from the primary key`}
                  onClick={() => onSetPrimaryKey(pk.filter((x) => x !== c))}
                  className="text-muted-foreground hover:text-foreground -mr-0.5 rounded-sm outline-none focus-visible:ring-2"
                >
                  <X className="size-3" />
                </button>
              )}
            </span>
          ))}
          {pk.length === 0 && !editable && (
            <span className="text-muted-foreground text-[11px] italic">none</span>
          )}
          {editable && candidates.length > 0 && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label="Add a column to the primary key"
                  className="text-muted-foreground hover:text-foreground hover:bg-accent flex size-5 items-center justify-center rounded-sm outline-none focus-visible:ring-2"
                >
                  <Plus className="size-3" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent className="max-h-72 min-w-36 overflow-auto">
                {candidates.map((c) => (
                  <DropdownMenuItem
                    key={c}
                    className="font-mono text-xs"
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
              "border-border min-w-32 flex-1 rounded-sm border",
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
              placeholder={editable ? "no comment" : undefined}
            />
          </div>
        </div>
      )}
      <span className={cn(dialect === "sqlite" && "flex-1")} />
      <div className="flex items-center gap-1">
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
          className="h-6 gap-1 px-2 text-xs"
        >
          <Zap className="size-3.5" />
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
          className="h-6"
        >
          <FileCode2 /> Info
        </Button>
      </div>
    </div>
  );
}

const ROW_FORMATS = ["DEFAULT", "DYNAMIC", "COMPACT", "REDUNDANT", "COMPRESSED"];

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
          className={cn("h-6", changed && "bg-changed")}
        >
          <Settings2 /> Options
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 gap-2.5 p-3">
        <p className="text-xs font-medium">Table options</p>
        <div className="grid grid-cols-[96px_1fr] items-center gap-x-2 gap-y-1.5 text-xs">
          <span className="text-muted-foreground text-[11px]">Engine</span>
          <ChoiceCell
            value={o?.engine ?? null}
            options={list(engines, o?.engine)}
            onChange={(v) => onSet("engine", v)}
            label="Engine"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono text-[11px]"
          />
          <span className="text-muted-foreground text-[11px]">Charset</span>
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
            className="border-border font-mono text-[11px]"
          />
          <span className="text-muted-foreground text-[11px]">Collation</span>
          <ChoiceCell
            value={collation}
            options={list(charset?.collations ?? [], collation)}
            onChange={(v) => onSet("collation", v)}
            label="Collation"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono text-[11px]"
          />
          <label htmlFor={aiId} className="text-muted-foreground text-[11px]">
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
            placeholder="next value"
            className="h-6 font-mono text-xs md:text-xs"
          />
          <span className="text-muted-foreground text-[11px]">Row format</span>
          <ChoiceCell
            value={rowFormat}
            options={list(ROW_FORMATS, rowFormat)}
            onChange={(v) => onSet("rowFormat", v)}
            label="Row format"
            disabled={!editable}
            placeholder="—"
            className="border-border font-mono text-[11px]"
          />
        </div>
        <p className="text-muted-foreground text-[11px]">
          A new charset or collation converts every text column (CONVERT TO CHARACTER SET).
        </p>
      </PopoverContent>
    </Popover>
  );
}
