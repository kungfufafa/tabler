import type { Value } from "@/lib/ipc";
import { findFreeSpot, GRID_COLUMNS, type Layout } from "./layout";

/**
 * The Metrics Board document (TablePlus's dashboards, spec §8.4): a board is a list of objects
 * on a 12-column grid. Charts, scoreboards, data tables and dropdowns (options from SQL) run an
 * SQL query; inputs and dropdowns set a variable (`{{name}}` in any object's SQL); inputs,
 * dropdowns and buttons refresh the objects wired to them (the green dot).
 */

export type ObjectKind =
  "bar" | "line" | "pie" | "scoreboard" | "table" | "input" | "dropdown" | "button";

export const OBJECT_KINDS: readonly ObjectKind[] = [
  "bar",
  "line",
  "pie",
  "scoreboard",
  "table",
  "input",
  "dropdown",
  "button",
];

/** TablePlus's refresh rates: After xx seconds / Refresh on event / Do not refresh. */
export type RefreshMode =
  { mode: "interval"; seconds: number } | { mode: "event" } | { mode: "none" };

export type InputType = "text" | "number" | "date";

export interface DropdownOption {
  value: string;
  label: string;
}

export interface BoardObject {
  id: string;
  kind: ObjectKind;
  title: string;
  layout: Layout;
  sql: string;
  refresh: RefreshMode;
  /** Run statements that change data (otherwise they are refused). */
  allowWrites: boolean;
  /**
   * "Query timeout" in seconds: a query still running after it is cancelled and the object shows
   * "Query timed out after N s" (`MIN_TIMEOUT`–`MAX_TIMEOUT`, `DEFAULT_TIMEOUT`).
   */
  timeoutSecs: number;
  /** Charts: the category (x) column; null = the result's first column. */
  x: string | null;
  /** Charts: the value (y) columns; empty = every numeric column but x. A pie uses the first. */
  y: string[];
  /** Charts: the palette slot (0–7) of each series, in order; missing = its own slot. */
  colors: number[];
  /** Scoreboard: the value column; null = the first one. */
  valueColumn: string | null;
  /** Scoreboard: an increase is good news (colours the change). */
  higherIsBetter: boolean;
  /** Input / dropdown: the variable it sets. */
  variable: string;
  inputType: InputType;
  defaultValue: string;
  /** Dropdown: its own option list, or its SQL (value column, then an optional label column). */
  optionsSource: "static" | "sql";
  options: DropdownOption[];
  /** Input / dropdown / button: the objects refreshed when it changes or is clicked. */
  targets: string[];
}

export interface KindInfo {
  label: string;
  /** Size of a new object, in grid cells. */
  size: { w: number; h: number };
  min: { w: number; h: number };
  /** Sets a variable / fires an event (has a green dot). */
  source: boolean;
  /** Draws a query result (charts, scoreboard, table). */
  data: boolean;
}

export const KIND_INFO: Record<ObjectKind, KindInfo> = {
  bar: { label: "Bar chart", size: { w: 6, h: 6 }, min: { w: 3, h: 4 }, source: false, data: true },
  line: {
    label: "Line chart",
    size: { w: 6, h: 6 },
    min: { w: 3, h: 4 },
    source: false,
    data: true,
  },
  pie: { label: "Pie chart", size: { w: 4, h: 6 }, min: { w: 3, h: 4 }, source: false, data: true },
  scoreboard: {
    label: "Scoreboard",
    size: { w: 3, h: 3 },
    min: { w: 2, h: 2 },
    source: false,
    data: true,
  },
  table: {
    label: "Data table",
    size: { w: 6, h: 6 },
    min: { w: 3, h: 3 },
    source: false,
    data: true,
  },
  input: {
    label: "Input field",
    size: { w: 3, h: 2 },
    min: { w: 2, h: 2 },
    source: true,
    data: false,
  },
  dropdown: {
    label: "Dropdown",
    size: { w: 3, h: 2 },
    min: { w: 2, h: 2 },
    source: true,
    data: false,
  },
  button: { label: "Button", size: { w: 2, h: 1 }, min: { w: 1, h: 1 }, source: true, data: false },
};

export const isChart = (kind: ObjectKind): kind is "bar" | "line" | "pie" =>
  kind === "bar" || kind === "line" || kind === "pie";

/** Whether the object runs its SQL on refresh (charts, scoreboard, table, SQL dropdown). */
export function runsQuery(o: BoardObject): boolean {
  if (o.sql.trim() === "") return false;
  if (KIND_INFO[o.kind].data) return true;
  return o.kind === "dropdown" && o.optionsSource === "sql";
}

/** The shortest refresh interval accepted, in seconds. */
export const MIN_INTERVAL = 1;
export const DEFAULT_INTERVAL = 60;

/** An object's "Query timeout" when it sets none (boards from before it existed), and its bounds. */
export const DEFAULT_TIMEOUT = 60;
export const MIN_TIMEOUT = 5;
export const MAX_TIMEOUT = 600;

const VARIABLE = /^[A-Za-z_][\w.]*$/;

/** A usable variable name (`{{name}}`: letters, digits, `_` and `.`). */
export function isVariableName(name: string): boolean {
  return VARIABLE.test(name);
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown, fallback = ""): string => (typeof v === "string" ? v : fallback);
const int = (v: unknown, fallback: number, min: number, max: number): number =>
  typeof v === "number" && Number.isFinite(v)
    ? Math.min(max, Math.max(min, Math.round(v)))
    : fallback;

function refreshOf(raw: unknown): RefreshMode {
  if (!isRecord(raw)) return { mode: "none" };
  if (raw.mode === "interval")
    return { mode: "interval", seconds: int(raw.seconds, DEFAULT_INTERVAL, MIN_INTERVAL, 86_400) };
  if (raw.mode === "event") return { mode: "event" };
  return { mode: "none" };
}

function optionsOf(raw: unknown): DropdownOption[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((o) =>
    isRecord(o) && typeof o.value === "string"
      ? [{ value: o.value, label: str(o.label, o.value) }]
      : [],
  );
}

/** A default object of `kind` (layout at the origin; `newObject` places it). */
export function defaultObject(kind: ObjectKind, id: string, title: string): BoardObject {
  const { size } = KIND_INFO[kind];
  return {
    id,
    kind,
    title,
    layout: { x: 0, y: 0, w: size.w, h: size.h },
    sql: "",
    refresh: { mode: "none" },
    allowWrites: false,
    timeoutSecs: DEFAULT_TIMEOUT,
    x: null,
    y: [],
    colors: [],
    valueColumn: null,
    higherIsBetter: true,
    variable: "",
    inputType: "text",
    defaultValue: "",
    optionsSource: "static",
    options: [],
    targets: [],
  };
}

/** One object read back from storage or a file; null when it is not one. */
function objectOf(raw: unknown): BoardObject | null {
  if (!isRecord(raw)) return null;
  const kind = raw.kind as ObjectKind;
  if (!OBJECT_KINDS.includes(kind) || typeof raw.id !== "string" || raw.id === "") return null;
  const base = defaultObject(kind, raw.id, str(raw.title, KIND_INFO[kind].label));
  const { min } = KIND_INFO[kind];
  const l = isRecord(raw.layout) ? raw.layout : {};
  const w = int(l.w, base.layout.w, min.w, GRID_COLUMNS);
  return {
    ...base,
    layout: {
      x: int(l.x, 0, 0, GRID_COLUMNS - w),
      y: int(l.y, 0, 0, 10_000),
      w,
      h: int(l.h, base.layout.h, min.h, 200),
    },
    sql: str(raw.sql),
    refresh: refreshOf(raw.refresh),
    allowWrites: raw.allowWrites === true,
    timeoutSecs: int(raw.timeoutSecs, DEFAULT_TIMEOUT, MIN_TIMEOUT, MAX_TIMEOUT),
    x: typeof raw.x === "string" && raw.x !== "" ? raw.x : null,
    y: Array.isArray(raw.y) ? raw.y.filter((v): v is string => typeof v === "string") : [],
    colors: Array.isArray(raw.colors) ? raw.colors.map((c) => int(c, 0, 0, 7)) : [],
    valueColumn:
      typeof raw.valueColumn === "string" && raw.valueColumn !== "" ? raw.valueColumn : null,
    higherIsBetter: raw.higherIsBetter !== false,
    variable: typeof raw.variable === "string" && isVariableName(raw.variable) ? raw.variable : "",
    inputType: raw.inputType === "number" || raw.inputType === "date" ? raw.inputType : "text",
    defaultValue: str(raw.defaultValue),
    optionsSource: raw.optionsSource === "sql" ? "sql" : "static",
    options: optionsOf(raw.options),
    targets: Array.isArray(raw.targets)
      ? raw.targets.filter((v): v is string => typeof v === "string")
      : [],
  };
}

/**
 * A board's objects as stored (app database, `.tablerboard` file): unknown kinds and malformed
 * entries are dropped, missing fields get their defaults, sizes are clamped to the grid, a
 * repeated id keeps its first object, and wires lead only to objects that exist.
 */
export function normalizeObjects(raw: unknown): BoardObject[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: BoardObject[] = [];
  for (const item of raw) {
    const o = objectOf(item);
    if (!o || seen.has(o.id)) continue;
    seen.add(o.id);
    out.push(o);
  }
  return out.map((o) => ({
    ...o,
    targets: [...new Set(o.targets)].filter((t) => t !== o.id && seen.has(t)),
  }));
}

/** `base 1`, `base 2`… : the first that no object uses yet (ignoring case). */
export function uniqueTitle(base: string, objects: readonly BoardObject[]): string {
  const taken = new Set(objects.map((o) => o.title.toLowerCase()));
  for (let i = 1; ; i++) {
    const title = `${base} ${i}`;
    if (!taken.has(title.toLowerCase())) return title;
  }
}

/** `input1`, `input2`… : a variable name no object sets yet. */
export function uniqueVariable(base: string, objects: readonly BoardObject[]): string {
  const taken = new Set(objects.map((o) => o.variable));
  for (let i = 1; ; i++) if (!taken.has(`${base}${i}`)) return `${base}${i}`;
}

/** A new object of `kind`, placed on the first free spot of the grid. */
export function newObject(
  kind: ObjectKind,
  objects: readonly BoardObject[],
  id: string = crypto.randomUUID(),
): BoardObject {
  const o = defaultObject(kind, id, uniqueTitle(KIND_INFO[kind].label, objects));
  const spot = findFreeSpot(
    objects.map((x) => x.layout),
    o.layout.w,
    o.layout.h,
  );
  o.layout = { ...o.layout, ...spot };
  if (kind === "input") o.variable = uniqueVariable("input", objects);
  if (kind === "dropdown") o.variable = uniqueVariable("dropdown", objects);
  if (kind === "button") o.title = uniqueTitle("Refresh", objects);
  return o;
}

/** A copy of `o` with a new id and title, placed on a free spot; its wires are not copied. */
export function duplicateObject(
  o: BoardObject,
  objects: readonly BoardObject[],
  id: string = crypto.randomUUID(),
): BoardObject {
  const spot = findFreeSpot(
    objects.map((x) => x.layout),
    o.layout.w,
    o.layout.h,
  );
  return {
    ...o,
    id,
    title: uniqueTitle(o.title.replace(/\s+\d+$/, ""), objects),
    layout: { ...o.layout, ...spot },
    variable: o.variable ? uniqueVariable(o.variable.replace(/\d+$/, ""), objects) : "",
    targets: [],
  };
}

/** Removes `id` and every wire that led to it. */
export function removeObject(objects: readonly BoardObject[], id: string): BoardObject[] {
  return objects
    .filter((o) => o.id !== id)
    .map((o) =>
      o.targets.includes(id) ? { ...o, targets: o.targets.filter((t) => t !== id) } : o,
    );
}

/**
 * Wires `sourceId` (input / dropdown / button) to `targetId`. The target must use "Refresh on
 * event" (TablePlus), so it is switched to it. Returns the objects unchanged when the wire is
 * not possible (or already there).
 */
export function wire(
  objects: readonly BoardObject[],
  sourceId: string,
  targetId: string,
): BoardObject[] {
  const source = objects.find((o) => o.id === sourceId);
  const target = objects.find((o) => o.id === targetId);
  if (!source || !target || source.id === target.id || !KIND_INFO[source.kind].source)
    return [...objects];
  if (!canWireTo(target)) return [...objects];
  return objects.map((o) => {
    if (o.id === sourceId && !o.targets.includes(targetId))
      return { ...o, targets: [...o.targets, targetId] };
    if (o.id === targetId && o.refresh.mode !== "event")
      return { ...o, refresh: { mode: "event" } };
    return o;
  });
}

export function unwire(
  objects: readonly BoardObject[],
  sourceId: string,
  targetId: string,
): BoardObject[] {
  return objects.map((o) =>
    o.id === sourceId ? { ...o, targets: o.targets.filter((t) => t !== targetId) } : o,
  );
}

/** Whether `target` can be wired to (it shows something a refresh changes). */
export function canWireTo(target: BoardObject): boolean {
  return KIND_INFO[target.kind].data || target.kind === "dropdown";
}

const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;

/**
 * The value an input sets from its text: a number input gives an integer or a decimal (NULL when
 * empty or not a number), a date input a date (NULL when empty), a text input the text itself.
 */
export function inputValue(type: InputType, text: string): Value {
  const t = text.trim();
  switch (type) {
    case "number": {
      if (!NUMBER.test(t)) return { t: "null" };
      const n = Number(t);
      return Number.isSafeInteger(n) && /^[+-]?\d+$/.test(t)
        ? { t: "int", v: n }
        : { t: "decimal", v: t };
    }
    case "date":
      return t === "" ? { t: "null" } : { t: "date", v: t };
    case "text":
      return { t: "text", v: text };
  }
}

/** Static dropdown options from the inspector's text: one per line, `value | label`. */
export function parseOptions(text: string): DropdownOption[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .map((line) => {
      const bar = line.indexOf("|");
      if (bar < 0) return { value: line, label: line };
      const value = line.slice(0, bar).trim();
      const label = line.slice(bar + 1).trim();
      return { value, label: label || value };
    });
}

export function formatOptions(options: readonly DropdownOption[]): string {
  return options.map((o) => (o.label === o.value ? o.value : `${o.value} | ${o.label}`)).join("\n");
}
