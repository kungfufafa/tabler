import type { Driver, ImportType, InsertMode, TextEncoding } from "@/lib/ipc";

/** The CSV wizard's matching strategies, as TablePlus names them. */
export type MatchStrategy = "nameAndOrder" | "nameAndOrderCi" | "name" | "nameCi" | "order";

export const MATCH_STRATEGIES: ReadonlyArray<{ value: MatchStrategy; label: string }> = [
  { value: "nameAndOrder", label: "Match Columns by Name and Order" },
  { value: "nameAndOrderCi", label: "Match Columns by Name and Order – Case insensitive" },
  { value: "name", label: "Match Columns by Name" },
  { value: "nameCi", label: "Match Columns by Name – Case insensitive" },
  { value: "order", label: "Match Columns by Order" },
];

export const INSERT_MODES: ReadonlyArray<{ value: InsertMode; label: string; title: string }> = [
  {
    value: "stopOnError",
    label: "STOP ON ERROR",
    title: "Insert the rows; the first error stops the import and rolls it back",
  },
  {
    value: "replace",
    label: "REPLACE",
    title: "Rows whose key exists replace the existing row",
  },
  {
    value: "updateOnDuplicate",
    label: "UPDATE ON DUPLICATE",
    title: "Rows whose key exists update the existing row",
  },
];

/**
 * Insert options for a connection: PostgreSQL has no REPLACE; a table the import creates has no
 * key, so only plain inserts.
 */
export function insertModesFor(driver: Driver | undefined, creating: boolean): InsertMode[] {
  if (creating) return ["stopOnError"];
  return driver === "postgres"
    ? ["stopOnError", "updateOnDuplicate"]
    : ["stopOnError", "replace", "updateOnDuplicate"];
}

export const COLUMN_TYPES: ReadonlyArray<{ value: ImportType; label: string }> = [
  { value: "integer", label: "integer" },
  { value: "bigint", label: "bigint" },
  { value: "numeric", label: "numeric" },
  { value: "boolean", label: "boolean" },
  { value: "date", label: "date" },
  { value: "timestamp", label: "timestamp" },
  { value: "timestampTz", label: "timestamptz" },
  { value: "text", label: "text" },
];

export const ENCODINGS: ReadonlyArray<{ value: TextEncoding; label: string }> = [
  { value: "utf8", label: "UTF-8" },
  { value: "utf16Le", label: "UTF-16 LE" },
  { value: "utf16Be", label: "UTF-16 BE" },
  { value: "windows1252", label: "Windows-1252" },
  { value: "latin1", label: "Latin-1 (ISO-8859-1)" },
];

export const DELIMITERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: ",", label: "Comma ( , )" },
  { value: ";", label: "Semicolon ( ; )" },
  { value: "\t", label: "Tab" },
  { value: "|", label: "Pipe ( | )" },
];

export const QUOTES: ReadonlyArray<{ value: string; label: string }> = [
  { value: '"', label: 'Double quote ( " )' },
  { value: "'", label: "Single quote ( ' )" },
];

/**
 * The table column each CSV column goes into (null = "Do not import"), by `strategy`:
 * - by name: the header's name equals the column's (case-insensitively for the "Case
 *   insensitive" strategies); without a header row nothing matches by name;
 * - by name and order: by name first, then each column left over by its position;
 * - by order: the CSV's n-th column into the table's n-th column.
 * A table column takes at most one CSV column (the first that matches).
 */
export function matchColumns(
  csv: readonly string[],
  table: readonly string[],
  strategy: MatchStrategy,
  header: boolean,
): (string | null)[] {
  const out: (string | null)[] = csv.map(() => null);
  const taken = new Set<string>();
  const byName = strategy !== "order" && header;
  const ci = strategy === "nameCi" || strategy === "nameAndOrderCi";
  if (byName) {
    csv.forEach((name, i) => {
      const want = ci ? name.trim().toLowerCase() : name.trim();
      const hit = table.find((t) => !taken.has(t) && (ci ? t.toLowerCase() : t) === want);
      if (hit !== undefined) {
        out[i] = hit;
        taken.add(hit);
      }
    });
  }
  const byOrder =
    strategy === "order" || strategy === "nameAndOrder" || strategy === "nameAndOrderCi";
  if (byOrder) {
    csv.forEach((_, i) => {
      const t = table[i];
      if (out[i] === null && t !== undefined && !taken.has(t)) {
        out[i] = t;
        taken.add(t);
      }
    });
  }
  return out;
}

/**
 * The matched mapping with the user's picks applied (CSV column index → table column or null).
 * A picked column is taken from the CSV column that had it, so no column is filled twice.
 */
export function applyOverrides(
  mapping: readonly (string | null)[],
  overrides: Readonly<Record<number, string | null>>,
): (string | null)[] {
  const out = [...mapping];
  for (const [key, value] of Object.entries(overrides)) {
    const i = Number(key);
    if (i < 0 || i >= out.length) continue;
    if (value !== null) {
      out.forEach((v, j) => {
        if (j !== i && v === value) out[j] = null;
      });
    }
    out[i] = value;
  }
  return out;
}

export function matchedCount(mapping: readonly (string | null)[]): number {
  return mapping.filter((m) => m !== null).length;
}

/** "CSV 5 columns, table 4 columns, matched 4 columns". */
export function statusLine(csv: number, table: number, matched: number): string {
  const n = (count: number) => `${count} column${count === 1 ? "" : "s"}`;
  return `CSV ${n(csv)}, table ${n(table)}, matched ${n(matched)}`;
}

/** A table name from a file name: `Sales Report 2026.csv` → `sales_report_2026`. */
export function tableNameFromFile(fileName: string): string {
  const stem = fileName.replace(/\.[^.]*$/, "").replace(/\.(csv|tsv|txt)$/i, "");
  let name = stem
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^_|_$/g, "");
  if (/^\d/.test(name)) name = `t_${name}`;
  return name || "imported";
}

// ---- remembered options ------------------------------------------------------------------------

export interface CsvWizardPrefs {
  header: boolean;
  quote: string;
  emptyAsNull: boolean;
  strategy: MatchStrategy;
}

export const DEFAULT_CSV_PREFS: Readonly<CsvWizardPrefs> = Object.freeze({
  header: true,
  quote: '"',
  emptyAsNull: true,
  strategy: "nameAndOrder",
});

export const CSV_PREFS_KEY = "tabler.import.csv";

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function storage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function loadCsvPrefs(s: StorageLike | null = storage()): CsvWizardPrefs {
  const d = DEFAULT_CSV_PREFS;
  try {
    const raw: unknown = JSON.parse(s?.getItem(CSV_PREFS_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null) return { ...d };
    const r = raw as Record<string, unknown>;
    return {
      header: typeof r.header === "boolean" ? r.header : d.header,
      quote: r.quote === "'" || r.quote === '"' ? r.quote : d.quote,
      emptyAsNull: typeof r.emptyAsNull === "boolean" ? r.emptyAsNull : d.emptyAsNull,
      strategy: MATCH_STRATEGIES.some((m) => m.value === r.strategy)
        ? (r.strategy as MatchStrategy)
        : d.strategy,
    };
  } catch {
    return { ...d };
  }
}

export function saveCsvPrefs(prefs: CsvWizardPrefs, s: StorageLike | null = storage()): void {
  try {
    s?.setItem(CSV_PREFS_KEY, JSON.stringify(prefs));
  } catch {
    // Storage blocked: the next import starts from the defaults.
  }
}
