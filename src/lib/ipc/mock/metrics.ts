// Mock handlers for the Metrics Board: metrics_list_boards, metrics_save_board,
// metrics_delete_board, metrics_export_boards, metrics_import_boards and metrics_run. Boards live
// in memory (seeded with a demo board on every connection); aggregate queries get made-up numbers
// that move between refreshes, plain SELECTs run through the mock run_sql over the fixtures.
import { splitStatements } from "@/features/editor/scanner";
import { allReads } from "@/features/safety/classify";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { MetricsBoard } from "../bindings/MetricsBoard";
import type { MetricsBoardInput } from "../bindings/MetricsBoardInput";
import type { MetricsQuery } from "../bindings/MetricsQuery";
import type { MetricsResult } from "../bindings/MetricsResult";
import type { QueryEvent } from "../bindings/QueryEvent";
import type { ResultColumn } from "../bindings/ResultColumn";
import type { Value } from "../bindings/Value";
// The query mock registers run_sql (plain SELECTs run there).
import "./query";
import { mockError, mockInvoke, registerMock } from "./transport";

const MAX_ROWS = 5_000;
const now = () => new Date().toISOString();

const DEMO_OBJECTS = [
  {
    id: "demo-users",
    kind: "scoreboard",
    title: "Users",
    layout: { x: 0, y: 0, w: 3, h: 3 },
    sql: "SELECT count(*) AS users FROM users",
    refresh: { mode: "interval", seconds: 10 },
  },
  {
    id: "demo-posts",
    kind: "scoreboard",
    title: "Posts published",
    layout: { x: 3, y: 0, w: 3, h: 3 },
    sql: "SELECT count(*) AS posts FROM posts WHERE published_at IS NOT NULL",
    refresh: { mode: "interval", seconds: 15 },
  },
  {
    id: "demo-role",
    kind: "dropdown",
    title: "Role",
    layout: { x: 6, y: 0, w: 3, h: 2 },
    variable: "role",
    optionsSource: "static",
    options: [
      { value: "admin", label: "Admin" },
      { value: "editor", label: "Editor" },
      { value: "viewer", label: "Viewer" },
    ],
    defaultValue: "editor",
    targets: ["demo-signups", "demo-table"],
  },
  {
    id: "demo-search",
    kind: "input",
    title: "Name contains",
    layout: { x: 9, y: 0, w: 3, h: 2 },
    variable: "q",
    inputType: "text",
    targets: ["demo-table"],
  },
  {
    id: "demo-refresh",
    kind: "button",
    title: "Refresh charts",
    layout: { x: 6, y: 2, w: 2, h: 1 },
    targets: ["demo-by-month", "demo-by-role"],
  },
  {
    id: "demo-by-month",
    kind: "bar",
    title: "Posts by month",
    layout: { x: 0, y: 3, w: 6, h: 6 },
    sql: "SELECT date_trunc('month', published_at) AS month, count(*) AS posts, count(DISTINCT user_id) AS authors FROM posts GROUP BY 1 ORDER BY 1",
    refresh: { mode: "event" },
    x: "month",
    y: ["posts", "authors"],
  },
  {
    id: "demo-signups",
    kind: "line",
    title: "Sign-ups per day",
    layout: { x: 6, y: 3, w: 6, h: 6 },
    sql: "SELECT date(created_at) AS day, count(*) AS signups FROM users WHERE role = {{role}} GROUP BY 1 ORDER BY 1",
    refresh: { mode: "event" },
  },
  {
    id: "demo-by-role",
    kind: "pie",
    title: "Users by role",
    layout: { x: 0, y: 9, w: 4, h: 6 },
    sql: "SELECT role, count(*) AS users FROM users GROUP BY role",
    refresh: { mode: "event" },
  },
  {
    id: "demo-table",
    kind: "table",
    title: "Latest users",
    layout: { x: 4, y: 9, w: 8, h: 6 },
    sql: "SELECT id, name, email, role FROM users WHERE name ILIKE '%' || {{q}} || '%' ORDER BY id DESC LIMIT 50",
    refresh: { mode: "event" },
  },
];

const boards = new Map<string, MetricsBoard>();
let order = 0;
const sortOrder = new Map<string, number>();

function seed(): void {
  const at = now();
  boards.set("demo-board", {
    id: "demo-board",
    profileId: null,
    name: "Sales overview",
    objects: DEMO_OBJECTS,
    createdAt: at,
    updatedAt: at,
  });
  sortOrder.set("demo-board", ++order);
}
seed();

/** Test-only: back to the demo board. */
export function resetMockMetrics(): void {
  boards.clear();
  sortOrder.clear();
  order = 0;
  seed();
}

function listFor(profileId: string | null): MetricsBoard[] {
  return [...boards.values()]
    .filter((b) => profileId === null || b.profileId === null || b.profileId === profileId)
    .sort((a, b) => (sortOrder.get(a.id) ?? 0) - (sortOrder.get(b.id) ?? 0));
}

function save(input: MetricsBoardInput): MetricsBoard {
  const name = input.name.trim();
  if (name === "") mockError("INVALID_INPUT", "The board needs a name");
  if (!Array.isArray(input.objects))
    mockError("INVALID_INPUT", "The board's objects must be a list");
  const id = input.id ?? crypto.randomUUID();
  const existing = boards.get(id);
  const at = now();
  const board: MetricsBoard = {
    id,
    profileId: input.profileId,
    name,
    objects: structuredClone(input.objects),
    createdAt: existing?.createdAt ?? at,
    updatedAt: at,
  };
  boards.set(id, board);
  if (!sortOrder.has(id)) sortOrder.set(id, ++order);
  return structuredClone(board);
}

registerMock("metrics_list_boards", ({ profileId }) =>
  structuredClone(listFor((profileId as string | null) ?? null)),
);
registerMock("metrics_save_board", ({ input }) => save(input as MetricsBoardInput));
registerMock("metrics_delete_board", ({ id }) => {
  boards.delete(String(id));
});

registerMock("metrics_export_boards", ({ ids, path }) => {
  const p = String(path ?? "");
  if (p.includes("missing")) mockError("IO", `${p}: No such file or directory (os error 2)`);
  const found = (ids as string[]).filter((id) => boards.has(id));
  if (found.length === 0) mockError("NOT_FOUND", "not found: board");
  return found.length;
});

/** `encrypted` in the path: the file needs the password "secret". */
registerMock("metrics_import_boards", ({ path, password, profileId }) => {
  const p = String(path ?? "");
  if (!p.endsWith(".tablerboard")) mockError("INVALID_INPUT", "This is not a Tabler board file");
  if (p.includes("encrypted")) {
    if (!password) return { needsPassword: true, boards: [] };
    if (password !== "secret")
      mockError("INVALID_INPUT", "The file password is wrong, or the file was modified");
  }
  const taken = new Set(listFor((profileId as string | null) ?? null).map((b) => b.name));
  let name = "Imported board";
  for (let i = 2; taken.has(name); i++) name = `Imported board ${i}`;
  const board = save({
    id: null,
    profileId: (profileId as string | null) ?? null,
    name,
    // As the backend: an imported object never keeps "Allow data changes".
    objects: DEMO_OBJECTS.slice(0, 2).map((o) => ({
      ...o,
      id: `${o.id}-${crypto.randomUUID()}`,
      allowWrites: false,
    })),
  });
  return { needsPassword: false, boards: [board] };
});

// ---- metrics_run --------------------------------------------------------------------------------

function hash(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** A pseudo-random number from `seed` (stable per seed). */
function rand(seed: number): number {
  const x = Math.sin(seed) * 10_000;
  return x - Math.floor(x);
}

function literal(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "NULL";
    case "int":
    case "float":
      return String(v.v);
    case "bool":
      return v.v ? "TRUE" : "FALSE";
    case "bytes":
      return "NULL";
    case "array":
    case "other":
      return "NULL";
    default:
      return `'${v.v.replace(/'/g, "''")}'`;
  }
}

function column(name: string, kind: ResultColumn["kind"], dataType: string): ResultColumn {
  return { name, dataType, kind, nullable: null, table: null };
}

/** The select list's column names (`AS alias`, else the expression). */
function selectNames(sql: string): string[] {
  const m = /^\s*select\s+([\s\S]*?)\s+from\s/i.exec(sql);
  if (!m) return [];
  const list: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of m[1]!) {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (ch === "," && depth === 0) {
      list.push(cur);
      cur = "";
    } else cur += ch;
  }
  list.push(cur);
  return list.map((expr) => {
    const alias = /\bas\s+"?([\w ]+?)"?\s*$/i.exec(expr.trim());
    return (alias?.[1] ?? expr.trim().split(".").pop() ?? "value").trim();
  });
}

const CATEGORIES = ["admin", "editor", "viewer", "guest", "owner"];

/** Functions a SELECT can call that change the server or hold a lock (the backend's list, cut). */
const SIDE_EFFECT_NAMES =
  "setval|nextval|pg_terminate_backend|pg_cancel_backend|pg_advisory_(?:xact_)?lock(?:_shared)?|pg_try_advisory_(?:xact_)?lock(?:_shared)?|pg_advisory_unlock(?:_shared|_all)?|lo_unlink|lo_import|dblink_exec|pg_sleep|get_lock|release_lock|sleep|benchmark|load_extension";
const SIDE_EFFECT_CALL = new RegExp(
  `\\b(?:(?:${SIDE_EFFECT_NAMES})\\s*\\(|next\\s+value\\s+for\\b)`,
  "i",
);

/**
 * A PostgreSQL `U&"d\0061ta"` name as the server reads it (`\XXXX`, `\+XXXXXX`, `\\`), or null
 * when it cannot be read here (a surrogate, a bad escape).
 */
function unicodeName(raw: string): string | null {
  let out = "";
  for (let i = 0; i < raw.length;) {
    if (raw[i] !== "\\") out += raw[i++];
    else if (raw[i + 1] === "\\") {
      out += "\\";
      i += 2;
    } else {
      const m = /^\\(?:\+([0-9a-f]{6})|([0-9a-f]{4}))/i.exec(raw.slice(i));
      const code = m ? parseInt(m[1] ?? m[2] ?? "", 16) : NaN;
      if (!m || Number.isNaN(code) || code > 0x10ffff || (code >= 0xd800 && code < 0xe000))
        return null;
      out += String.fromCodePoint(code);
      i += m[0].length;
    }
  }
  return out;
}

/**
 * The statement as the server reads it when it looks for a call, as the backend's classifier
 * does: MySQL executable comments and hints (opened by an exclamation mark or a plus sign,
 * with the version number) are code and the end of the comment is a space; PostgreSQL `U&"…"`
 * names are decoded (one with a `UESCAPE` clause, or that cannot be read, stands for a function
 * of the list); quoted names are plain names.
 */
function asCalled(sql: string, driver: string): string {
  let text = sql;
  if (driver === "mysql") text = text.replace(/\/\*(?:m?!\d{5,}|m?!|\+)/gi, " ");
  text = text.replace(/\*\//g, " ");
  if (driver === "postgres")
    text = text.replace(
      /\bu&"((?:[^"]|"")*)"(\s*uescape\s*(?:'[^']*'|\$[a-z_]*\$.*?\$[a-z_]*\$|e'[^']*'))?/gi,
      (_all, raw: string, clause: string | undefined) =>
        ` ${(clause ? null : unicodeName(raw)) ?? "pg_sleep"} `,
    );
  return text.replace(/["`[\]]/g, " ");
}

/** The seconds a made-up `pg_sleep(n)` / `SLEEP(n)` takes (the mock only pretends to wait). */
function sleepSeconds(sql: string): number {
  const m = /\b(?:pg_sleep|sleep)\s*\(\s*(\d+(?:\.\d+)?)\s*\)/i.exec(sql);
  return m ? Number(m[1]) : 0;
}

/** Made-up rows for an aggregate query: one row, or one per group (months, days, categories). */
function synthesize(sql: string): { columns: ResultColumn[]; rows: Value[][] } | null {
  if (!/\b(count|sum|avg|min|max)\s*\(/i.test(sql)) return null;
  const names = selectNames(sql);
  if (names.length === 0) return null;
  const seed = hash(sql);
  const tick = Math.floor(Date.now() / 5_000);
  if (!/\bgroup\s+by\b/i.test(sql)) {
    const columns = names.map((n) => column(n, "bigInt", "int8"));
    const row: Value[] = names.map((_, i) => {
      const base = 100 + (hash(`${seed}:${i}`) % 5_000);
      return { t: "int", v: base + Math.floor(rand(tick + i) * 40) };
    });
    return { columns, rows: [row] };
  }
  const [xName, ...yNames] = names;
  const temporal = /month|date|day|week|year|created|published|time/i.test(xName ?? "");
  const n = temporal ? 12 : 4;
  const columns = [
    column(xName ?? "x", temporal ? "date" : "text", temporal ? "date" : "text"),
    ...yNames.map((y) => column(y, "bigInt", "int8")),
  ];
  const rows: Value[][] = [];
  for (let i = 0; i < n; i++) {
    const x: Value = temporal
      ? {
          t: "date",
          v: new Date(
            Date.UTC(
              2026,
              /month/i.test(xName ?? "") ? i : 9,
              /month/i.test(xName ?? "") ? 1 : i + 1,
            ),
          )
            .toISOString()
            .slice(0, 10),
        }
      : { t: "text", v: CATEGORIES[i] ?? `group ${i + 1}` };
    rows.push([
      x,
      ...yNames.map((_, j): Value => ({
        t: "int",
        v: Math.round(20 + rand((seed % 997) + i * 7 + j * 31) * 180 + rand(tick + i) * 10),
      })),
    ]);
  }
  return { columns, rows };
}

registerMock("metrics_run", async ({ connectionId, queryId, query }) => {
  const q = query as MetricsQuery;
  if (q.parts.length !== q.values.length + 1)
    mockError("INVALID_INPUT", "the query's variables do not match their values");
  const sql = q.parts.reduce((acc, part, i) => {
    const v = q.values[i - 1];
    return acc + (v ? ` ${literal(v)} ` : "") + part;
  }, "");
  const list = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const info = list.find((c) => c.connectionId === connectionId);
  if (!info) mockError("NOT_CONNECTED", "not connected");
  const driver = info.driver;
  const statements = splitStatements(sql, driver);
  if (statements.length === 0) mockError("INVALID_INPUT", "The query is empty");
  if (statements.length > 1)
    mockError("INVALID_INPUT", "A board object runs one statement. Remove the others.");
  const text = sql.slice(statements[0]!.start, statements[0]!.end);
  const writes = !allReads([text], driver);
  if (writes && !q.allowWrites)
    mockError(
      "INVALID_INPUT",
      'This query changes data. Turn on "Allow data changes" in the object\'s settings to run it.',
    );
  if (!q.allowWrites && SIDE_EFFECT_CALL.test(asCalled(text, driver)))
    mockError(
      "INVALID_INPUT",
      "This query calls a function that changes data or holds a lock. Turn on 'Allow data changes' in the object's settings to run it.",
    );
  // The object's "Query timeout" (5 to 600 s, 60 by default), as the backend enforces it: reads
  // and writes alike are cancelled once they outlive it.
  const limit = Math.min(600, Math.max(5, Math.round(q.timeoutSecs ?? 60)));
  if (sleepSeconds(asCalled(text, driver)) > limit) {
    await new Promise((r) => setTimeout(r, 30));
    mockError("DB_ERROR", `Query timed out after ${limit} s`);
  }
  const t0 = performance.now();
  await new Promise((r) => setTimeout(r, 30 + Math.random() * 120));
  const maxRows = Math.min(q.maxRows || MAX_ROWS, MAX_ROWS);
  const done = (columns: ResultColumn[], rows: Value[][], affected: number | null) =>
    ({
      columns,
      rows: rows.slice(0, maxRows),
      truncated: rows.length > maxRows,
      affected,
      durationMs: Math.max(1, Math.round(performance.now() - t0)),
      sql: text,
    }) satisfies MetricsResult;
  if (writes) return done([], [], 1);
  const made = synthesize(text);
  if (made) return done(made.columns, made.rows, null);
  // Anything else runs through the mock SQL editor (errors come back in-band).
  let columns: ResultColumn[] = [];
  const rows: Value[][] = [];
  let error: string | null = null;
  await mockInvoke("run_sql", {
    connectionId,
    queryId,
    sql: text,
    options: { maxRows: maxRows + 1 },
    onEvent: {
      send: (e: QueryEvent) => {
        if (e.event === "columns" && e.data.set === 0) columns = e.data.columns;
        if (e.event === "rows" && e.data.set === 0) rows.push(...e.data.rows);
        if (e.event === "failed") error = e.data.error.message;
      },
    },
  });
  if (error) mockError("DB", error);
  return done(columns, rows, null);
});
