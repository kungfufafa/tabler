import { Copy, Play, Trash2, X } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import type { Paint } from "@/features/charts/scene";
import { paintVar } from "@/features/charts/svg";
import type { Dialect } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { isNumericKind } from "@/lib/values";
import { cn } from "@/lib/utils";
import { BoardSqlEditor } from "./BoardSqlEditor";
import {
  DEFAULT_INTERVAL,
  formatOptions,
  isChart,
  isVariableName,
  KIND_INFO,
  MAX_TIMEOUT,
  MIN_INTERVAL,
  MIN_TIMEOUT,
  parseOptions,
  unwire,
  type BoardObject,
  type InputType,
  type RefreshMode,
} from "./model";
import { chartMapping } from "./results";
import { runKey, updateObject, updateObjects, useMetricsStore } from "./store";

const FIRST = "\u0000first";
const SLOTS = [0, 1, 2, 3, 4, 5, 6, 7];

function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-muted-foreground text-[11px] font-medium">{label}</span>
      {children}
      {hint && <p className="text-muted-foreground text-[10px] leading-snug">{hint}</p>}
    </div>
  );
}

function Toggle({
  label,
  checked,
  onChange,
  hint,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  hint?: string;
}) {
  const id = useId();
  return (
    <div className="flex flex-col gap-0.5">
      <label htmlFor={id} className="flex items-center justify-between gap-2 text-xs">
        {label}
        <Switch id={id} size="sm" checked={checked} onCheckedChange={onChange} />
      </label>
      {hint && <p className="text-muted-foreground text-[10px] leading-snug">{hint}</p>}
    </div>
  );
}

function refreshKey(r: RefreshMode): string {
  return r.mode;
}

/** After xx seconds / Refresh on event / Do not refresh. */
function RefreshField({
  object,
  onChange,
}: {
  object: BoardObject;
  onChange: (r: RefreshMode) => void;
}) {
  const r = object.refresh;
  return (
    <Field
      label="Refresh"
      hint={
        r.mode === "event"
          ? "Runs again when a wired input, dropdown or button fires."
          : r.mode === "none"
            ? `Runs when the board opens and on Refresh (${formatCombo(shortcutFor("workspace.reload") ?? "Mod+R")}).`
            : undefined
      }
    >
      <div className="flex items-center gap-1.5">
        <Select
          value={refreshKey(r)}
          onValueChange={(mode) =>
            onChange(
              mode === "interval"
                ? { mode: "interval", seconds: DEFAULT_INTERVAL }
                : mode === "event"
                  ? { mode: "event" }
                  : { mode: "none" },
            )
          }
        >
          <SelectTrigger size="sm" className="min-w-0 flex-1 text-xs" aria-label="Refresh">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="interval" className="text-xs">
              After N seconds
            </SelectItem>
            <SelectItem value="event" className="text-xs">
              Refresh on event
            </SelectItem>
            <SelectItem value="none" className="text-xs">
              Do not refresh
            </SelectItem>
          </SelectContent>
        </Select>
        {r.mode === "interval" && (
          <Input
            type="number"
            min={MIN_INTERVAL}
            aria-label="Seconds"
            className="h-7 w-16 text-xs md:text-xs"
            value={r.seconds}
            onChange={(e) => {
              const n = Math.round(Number(e.target.value));
              if (Number.isFinite(n) && n >= MIN_INTERVAL)
                onChange({ mode: "interval", seconds: Math.min(n, 86_400) });
            }}
          />
        )}
      </div>
    </Field>
  );
}

/** "Query timeout": seconds after which the object's query is cancelled (5 to 600). */
function TimeoutField({
  seconds,
  onChange,
}: {
  seconds: number;
  onChange: (seconds: number) => void;
}) {
  const [text, setText] = useState(String(seconds));
  const inRange = (n: number) => Number.isInteger(n) && n >= MIN_TIMEOUT && n <= MAX_TIMEOUT;
  return (
    <Field
      label="Query timeout (seconds)"
      hint={`A query still running after this is cancelled and the object says so. ${MIN_TIMEOUT} to ${MAX_TIMEOUT} seconds.`}
    >
      <Input
        type="number"
        min={MIN_TIMEOUT}
        max={MAX_TIMEOUT}
        aria-label="Query timeout"
        className="h-7 w-24 text-xs md:text-xs"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          const n = Number(e.target.value);
          if (e.target.value.trim() !== "" && inRange(n)) onChange(n);
        }}
        onBlur={() => {
          // Whatever is left in the box (empty, 3, 9000) becomes the nearest valid value.
          const n = Math.round(Number(text));
          const next =
            text.trim() === "" || !Number.isFinite(n)
              ? seconds
              : Math.min(MAX_TIMEOUT, Math.max(MIN_TIMEOUT, n));
          setText(String(next));
          if (next !== seconds) onChange(next);
        }}
      />
    </Field>
  );
}

function ColumnSelect({
  label,
  value,
  columns,
  firstLabel,
  onChange,
}: {
  label: string;
  value: string | null;
  columns: readonly string[];
  firstLabel: string;
  onChange: (v: string | null) => void;
}) {
  const list = value !== null && !columns.includes(value) ? [value, ...columns] : columns;
  return (
    <Field label={label}>
      <Select value={value ?? FIRST} onValueChange={(v) => onChange(v === FIRST ? null : v)}>
        <SelectTrigger size="sm" className="w-full text-xs" aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={FIRST} className="text-xs">
            {firstLabel}
          </SelectItem>
          {list.map((c) => (
            <SelectItem key={c} value={c} className="font-mono text-xs">
              {c}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </Field>
  );
}

/** A series' colour: one of the chart palette's eight slots. */
function SwatchRow({
  name,
  slot,
  onPick,
}: {
  name: string;
  slot: number;
  onPick: (slot: number) => void;
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="min-w-0 flex-1 truncate font-mono text-[11px]">{name}</span>
      <div role="radiogroup" aria-label={`Colour of ${name}`} className="flex gap-0.5">
        {SLOTS.map((s) => (
          <button
            key={s}
            type="button"
            role="radio"
            aria-checked={s === slot}
            aria-label={`Colour ${s + 1}`}
            onClick={() => onPick(s)}
            className={cn(
              "ring-offset-surface size-3.5 rounded-[3px] ring-offset-1",
              s === slot && "ring-foreground ring-2",
            )}
            style={{ background: paintVar(`series-${s + 1}` as Paint) }}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * The selected object's settings (edit mode): title, SQL (the app's CodeMirror setup), refresh
 * rate, "Allow data changes", chart mappings and colours, scoreboard / input / dropdown options,
 * and the objects it refreshes.
 */
export function Inspector({
  boardId,
  object,
  objects,
  dialect,
  onRun,
  onDuplicate,
  onDelete,
  onClose,
}: {
  boardId: string;
  object: BoardObject;
  objects: readonly BoardObject[];
  dialect: Dialect;
  onRun: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  const result = useMetricsStore((s) => s.runs[runKey(boardId, object.id)]?.result ?? null);
  const set = (patch: Partial<BoardObject>) => updateObject(boardId, object.id, patch);
  const info = KIND_INFO[object.kind];
  const columns = result?.columns ?? [];
  const names = columns.map((c) => c.name);
  const [variable, setVariableText] = useState(object.variable);
  const variableTaken = objects.some(
    (o) => o.id !== object.id && o.variable !== "" && o.variable === variable,
  );
  const variableError =
    variable === ""
      ? "Name the variable"
      : !isVariableName(variable)
        ? "Letters, digits, _ and . only (not starting with a digit)"
        : variableTaken
          ? "Another input or dropdown sets this variable"
          : null;
  const [optionsText, setOptionsText] = useState(() => formatOptions(object.options));

  const hasSql =
    info.data ||
    object.kind === "button" ||
    (object.kind === "dropdown" && object.optionsSource === "sql");
  const scheduled = info.data || (object.kind === "dropdown" && object.optionsSource === "sql");
  const variables = objects.filter((o) => o.variable).map((o) => `{{${o.variable}}}`);
  const chart = isChart(object.kind);
  // The series drawn: the mapped y columns, else every numeric column but x (as the chart does).
  const mapping = chartMapping(object);
  const xName = mapping.x ?? columns[0]?.name;
  const series = (
    mapping.y.length > 0
      ? mapping.y
      : columns.filter((c) => isNumericKind(c.kind) && c.name !== xName).map((c) => c.name)
  ).slice(0, object.kind === "pie" ? 1 : 8);

  const sqlHint =
    object.kind === "dropdown"
      ? "SELECT id, name FROM customers ORDER BY name"
      : object.kind === "button"
        ? "UPDATE orders SET status = 'shipped' WHERE id = {{order}}"
        : object.kind === "scoreboard"
          ? "SELECT count(*) AS orders FROM orders"
          : "SELECT date(created_at) AS day, count(*) AS orders FROM orders GROUP BY 1";

  return (
    <aside
      aria-label={`${info.label} settings`}
      data-slot="metrics-inspector"
      className="border-border bg-surface-2 flex h-full w-72 shrink-0 flex-col border-l"
    >
      <div className="border-border flex h-9 shrink-0 items-center gap-2 border-b px-3">
        <span className="min-w-0 flex-1 truncate text-xs font-semibold">{info.label}</span>
        <Button variant="ghost" size="icon-xs" aria-label="Close settings" onClick={onClose}>
          <X />
        </Button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-3">
        <Field label={object.kind === "button" ? "Label" : "Title"}>
          <Input
            value={object.title}
            aria-label="Title"
            className="h-7 text-xs md:text-xs"
            onChange={(e) => set({ title: e.target.value })}
          />
        </Field>

        {(object.kind === "input" || object.kind === "dropdown") && (
          <Field
            label="Variable"
            hint={variableError ?? `Use {{${variable}}} in any object's SQL.`}
          >
            <Input
              value={variable}
              aria-label="Variable"
              aria-invalid={variableError !== null}
              className="h-7 font-mono text-xs md:text-xs"
              onChange={(e) => {
                const v = e.target.value.trim();
                setVariableText(v);
                const taken = objects.some((o) => o.id !== object.id && o.variable === v);
                if (isVariableName(v) && !taken) set({ variable: v });
              }}
            />
          </Field>
        )}

        {object.kind === "input" && (
          <>
            <Field label="Type">
              <Select
                value={object.inputType}
                onValueChange={(v) => set({ inputType: v as InputType, defaultValue: "" })}
              >
                <SelectTrigger size="sm" className="w-full text-xs" aria-label="Type">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="text" className="text-xs">
                    Text
                  </SelectItem>
                  <SelectItem value="number" className="text-xs">
                    Number
                  </SelectItem>
                  <SelectItem value="date" className="text-xs">
                    Date
                  </SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <Field label="Default value">
              <Input
                type={
                  object.inputType === "number"
                    ? "number"
                    : object.inputType === "date"
                      ? "date"
                      : "text"
                }
                value={object.defaultValue}
                aria-label="Default value"
                className="h-7 text-xs md:text-xs"
                onChange={(e) => set({ defaultValue: e.target.value })}
              />
            </Field>
          </>
        )}

        {object.kind === "dropdown" && (
          <Field label="Options">
            <Select
              value={object.optionsSource}
              onValueChange={(v) =>
                set({
                  optionsSource: v === "sql" ? "sql" : "static",
                  ...(v === "sql" ? {} : { refresh: { mode: "none" } }),
                })
              }
            >
              <SelectTrigger size="sm" className="w-full text-xs" aria-label="Options">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="static" className="text-xs">
                  Static list
                </SelectItem>
                <SelectItem value="sql" className="text-xs">
                  Options from SQL
                </SelectItem>
              </SelectContent>
            </Select>
          </Field>
        )}
        {object.kind === "dropdown" && object.optionsSource === "static" && (
          <Field label="Option list" hint="One per line: value, or value | label.">
            <Textarea
              value={optionsText}
              aria-label="Option list"
              rows={5}
              className="min-h-20 font-mono text-xs md:text-xs"
              onChange={(e) => {
                setOptionsText(e.target.value);
                set({ options: parseOptions(e.target.value) });
              }}
            />
          </Field>
        )}
        {object.kind === "dropdown" && (
          <Field label="Default value" hint="The option chosen at first (else the first one).">
            <Input
              value={object.defaultValue}
              aria-label="Default value"
              className="h-7 text-xs md:text-xs"
              onChange={(e) => set({ defaultValue: e.target.value })}
            />
          </Field>
        )}

        {hasSql && (
          <Field
            label={
              object.kind === "button"
                ? "SQL (runs when clicked, optional)"
                : object.kind === "dropdown"
                  ? "SQL (value column, then label column)"
                  : "SQL query"
            }
            hint={
              variables.length > 0
                ? `Variables: ${variables.join(", ")}. Write '%' || {{q}} || '%', not '%{{q}}%'.`
                : "Add an input field or dropdown to use {{variables}}."
            }
          >
            <BoardSqlEditor
              key={object.id}
              value={object.sql}
              dialect={dialect}
              label={`SQL of ${object.title}`}
              hint={sqlHint}
              onChange={(sql) => set({ sql })}
              onRun={onRun}
            />
          </Field>
        )}

        {scheduled && <RefreshField object={object} onChange={(refresh) => set({ refresh })} />}

        {hasSql && (
          <TimeoutField
            seconds={object.timeoutSecs}
            onChange={(timeoutSecs) => set({ timeoutSecs })}
          />
        )}

        {hasSql && (
          <Toggle
            label="Allow data changes"
            checked={object.allowWrites}
            onChange={(allowWrites) => set({ allowWrites })}
            hint="Off: statements that change data are refused. On: they run when you click Refresh or a button, after the connection's safe-mode check; never by themselves when the board opens or on a timer."
          />
        )}

        {chart && (
          <>
            <ColumnSelect
              label={object.kind === "pie" ? "Label column (x)" : "X column"}
              value={object.x}
              columns={names}
              firstLabel="First column"
              onChange={(x) => set({ x })}
            />
            <Field
              label={object.kind === "pie" ? "Value column (y)" : "Y columns"}
              hint={
                object.y.length === 0
                  ? "None picked: every numeric column but x."
                  : names.length === 0
                    ? "Run the query to pick from its columns."
                    : undefined
              }
            >
              <div className="flex flex-col gap-1">
                {(names.length > 0 ? names : object.y).map((name) => (
                  <label key={name} className="flex items-center gap-2 text-xs">
                    <Checkbox
                      checked={object.y.includes(name)}
                      onCheckedChange={(v) => {
                        const on = v === true;
                        const y =
                          object.kind === "pie"
                            ? on
                              ? [name]
                              : []
                            : on
                              ? [...object.y, name]
                              : object.y.filter((n) => n !== name);
                        set({ y });
                      }}
                    />
                    <span className="truncate font-mono">{name}</span>
                  </label>
                ))}
              </div>
            </Field>
            {series.length > 0 && object.kind !== "pie" && (
              <Field label="Colours">
                <div className="tabler-chart flex flex-col gap-1">
                  {series.map((name, i) => (
                    <SwatchRow
                      key={name}
                      name={name}
                      slot={object.colors[i] ?? i % 8}
                      onPick={(slot) => {
                        const colors = series.map((_, j) => object.colors[j] ?? j % 8);
                        colors[i] = slot;
                        set({ colors });
                      }}
                    />
                  ))}
                </div>
              </Field>
            )}
          </>
        )}

        {object.kind === "scoreboard" && (
          <>
            <ColumnSelect
              label="Value column"
              value={object.valueColumn}
              columns={names}
              firstLabel="First column"
              onChange={(valueColumn) => set({ valueColumn })}
            />
            <Toggle
              label="Higher is better"
              checked={object.higherIsBetter}
              onChange={(higherIsBetter) => set({ higherIsBetter })}
              hint="Colours the change since the previous refresh."
            />
          </>
        )}

        {info.source && (
          <Field
            label={object.kind === "button" ? "Refreshes when clicked" : "Refreshes when changed"}
            hint="Drag the green dot onto an object to wire it (it switches to Refresh on event)."
          >
            {object.targets.length === 0 ? (
              <p className="text-muted-foreground text-[11px] italic">Nothing wired yet</p>
            ) : (
              <ul className="flex flex-col gap-0.5">
                {object.targets.map((id) => {
                  const target = objects.find((o) => o.id === id);
                  return (
                    <li key={id} className="flex items-center gap-1 text-xs">
                      <span className="bg-success size-1.5 shrink-0 rounded-full" />
                      <span className="min-w-0 flex-1 truncate">{target?.title ?? id}</span>
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`Unwire ${target?.title ?? id}`}
                        onClick={() =>
                          updateObjects(boardId, (list) => unwire(list, object.id, id))
                        }
                      >
                        <X />
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
          </Field>
        )}
      </div>
      <div className="border-border flex shrink-0 items-center gap-1 border-t px-2 py-1.5">
        {hasSql && (
          <Button size="xs" variant="outline" onClick={onRun} disabled={object.sql.trim() === ""}>
            <Play />
            {object.kind === "button" ? "Click" : "Run"}
          </Button>
        )}
        <span className="flex-1" />
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label="Duplicate"
          title="Duplicate"
          onClick={onDuplicate}
        >
          <Copy />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label="Delete"
          title="Delete"
          onClick={onDelete}
        >
          <Trash2 />
        </Button>
      </div>
    </aside>
  );
}
