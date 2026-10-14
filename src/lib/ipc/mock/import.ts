// Mock handlers for File ▸ Import: preview_csv_import, import_csv, preview_json_import,
// import_json, preview_sql_dump, import_sql_dump and cancel_import. The "files" are generated from their names, so the browser
// mock shows a realistic wizard: `users.csv` matches the seeded users table, anything else is a
// small product list; a name containing `bad` fails partway, `latin` is detected as Windows-1252.
import type { CsvImportProgress } from "../bindings/CsvImportProgress";
import type { CsvImportReport } from "../bindings/CsvImportReport";
import type { CsvImportRequest } from "../bindings/CsvImportRequest";
import type { CsvPreview } from "../bindings/CsvPreview";
import type { CsvPreviewRequest } from "../bindings/CsvPreviewRequest";
import type { ImportColumnDef } from "../bindings/ImportColumnDef";
import type { ImportType } from "../bindings/ImportType";
import type { JsonImportRequest } from "../bindings/JsonImportRequest";
import type { JsonPreview } from "../bindings/JsonPreview";
import type { JsonPreviewRequest } from "../bindings/JsonPreviewRequest";
import type { SqlDumpPreview } from "../bindings/SqlDumpPreview";
import type { SqlImportProgress } from "../bindings/SqlImportProgress";
import type { SqlImportReport } from "../bindings/SqlImportReport";
import type { SqlImportRequest } from "../bindings/SqlImportRequest";
import { mockError, mockInvoke, mockLatency, registerMock } from "./transport";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const running = new Set<string>();
const cancelled = new Set<string>();

const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

interface MockCsv {
  header: string[];
  types: ImportType[];
  rows: string[][];
}

/** The rows of a generated CSV file, from its name. */
function csvFile(path: string): MockCsv {
  const name = baseName(path).toLowerCase();
  if (!path.startsWith("/")) mockError("INVALID_INPUT", "the file path must be absolute");
  if (name.includes("missing")) mockError("IO", `cannot open "${path}": No such file or directory`);
  if (name.includes("users")) {
    const roles = ["admin", "editor", "viewer"];
    return {
      header: ["id", "email", "name", "role", "is_active", "balance", "created_at"],
      types: ["integer", "text", "text", "text", "boolean", "numeric", "timestamp"],
      rows: Array.from({ length: 240 }, (_, i) => [
        String(1000 + i),
        `user${i}@example.com`,
        i % 7 === 0 ? `"Doe, Jane ${i}"` : `User ${i}`,
        roles[i % 3]!,
        i % 4 === 0 ? "false" : "true",
        (i * 13.37).toFixed(2),
        `2026-0${(i % 9) + 1}-1${i % 9} 10:${String(i % 60).padStart(2, "0")}:00`,
      ]),
    };
  }
  return {
    header: ["sku", "Product Name", "price", "in_stock", "added"],
    types: ["text", "text", "numeric", "integer", "date"],
    rows: Array.from({ length: 64 }, (_, i) => [
      `SKU-${String(i).padStart(4, "0")}`,
      i % 5 === 0 ? "" : `Product ${i}`,
      (4.5 + i * 1.25).toFixed(2),
      String((i * 7) % 40),
      `2026-02-${String((i % 27) + 1).padStart(2, "0")}`,
    ]),
  };
}

function column(name: string, ty: ImportType): ImportColumnDef {
  return {
    name,
    ty,
    precision: ty === "numeric" ? 8 : null,
    scale: ty === "numeric" ? 2 : null,
  };
}

/** A field of the generated file: `"…"` is quoted, an empty string is an unquoted empty field. */
const cell = (raw: string): string | null =>
  raw === "" ? null : raw.startsWith('"') ? raw.slice(1, -1) : raw;

registerMock("preview_csv_import", async ({ req }) => {
  const r = req as CsvPreviewRequest;
  const file = csvFile(r.path);
  const name = baseName(r.path);
  await sleep(mockLatency === 0 ? 0 : 120);
  const delimiter = r.format.delimiter ?? (name.endsWith(".tsv") ? "\t" : ",");
  const encoding =
    r.format.encoding ?? (name.toLowerCase().includes("latin") ? "windows1252" : "utf8");
  // Another delimiter than the file's: every line is one field.
  const fileDelimiter = name.endsWith(".tsv") ? "\t" : ",";
  const lines = [file.header, ...file.rows];
  const records = delimiter === fileDelimiter ? lines : lines.map((l) => [l.join(fileDelimiter)]);
  const head = r.format.header ? records[0]! : null;
  const data = r.format.header ? records.slice(1) : records;
  const width = Math.max(head?.length ?? 0, ...data.map((d) => d.length));
  const columns = Array.from({ length: width }, (_, i) => {
    const colName = head?.[i]?.trim() || `column_${i + 1}`;
    const ty: ImportType =
      delimiter === fileDelimiter && r.format.header ? (file.types[i] ?? "text") : "text";
    return { name: colName, detected: column(colName, ty) };
  });
  return {
    path: r.path,
    fileName: name,
    size: lines.reduce((n, l) => n + l.join(",").length + 1, 0),
    delimiter,
    encoding,
    columns,
    rows: data.slice(0, 100).map((row) => row.map(cell)),
    sampledRows: Math.min(1000, data.length),
    more: data.length > 100,
  } satisfies CsvPreview;
});

registerMock("import_csv", async ({ req, onProgress }) => {
  const r = req as CsvImportRequest;
  const channel = onProgress as { send?: (p: CsvImportProgress) => void } | undefined;
  if (!r.importId?.trim()) mockError("INVALID_INPUT", "importId is required");
  if (running.has(r.importId))
    mockError("INVALID_INPUT", `import "${r.importId}" is already running`);
  if (!r.mapping.some((m) => m !== null))
    mockError("INVALID_INPUT", "no CSV column is imported into a table column");
  const file = csvFile(r.path);
  const total = r.format.header ? file.rows.length : file.rows.length + 1;
  const bad = baseName(r.path).toLowerCase().includes("bad");
  const failAt = bad ? Math.min(total, 42) : null;
  const totalBytes = total * 70;
  const started = performance.now();
  const steps = 8;
  running.add(r.importId);
  try {
    let read = 0;
    for (let i = 1; i <= steps; i++) {
      await sleep(mockLatency === 0 ? 1 : 90);
      if (cancelled.has(r.importId)) {
        return {
          rowsImported: 0,
          rowsRead: read,
          singleTransaction: true,
          createdTable: false,
          cancelled: true,
          failure: null,
          rowsSkipped: 0,
          skipped: [],
          notes: [],
          durationMs: Math.round(performance.now() - started),
        } satisfies CsvImportReport;
      }
      read = Math.round((total * i) / steps);
      channel?.send?.({
        phase: i < steps ? "reading" : "writing",
        rowsRead: read,
        rowsImported: 0,
        bytes: Math.round((totalBytes * i) / steps),
        totalBytes,
      });
    }
    await sleep(mockLatency === 0 ? 1 : 150);
    const failure =
      failAt === null
        ? null
        : {
            message:
              r.target.kind === "existing"
                ? `null value in column "${r.mapping.find((m) => m !== null)}" violates not-null constraint`
                : 'invalid input syntax for type integer: "n/a"',
            detail: null,
            firstRow: 1,
            lastRow: total,
            line: r.format.header ? 2 : 1,
          };
    const imported = failure ? 0 : total;
    channel?.send?.({
      phase: "writing",
      rowsRead: total,
      rowsImported: imported,
      bytes: totalBytes,
      totalBytes,
    });
    return {
      rowsImported: imported,
      rowsRead: total,
      singleTransaction: true,
      createdTable: r.target.kind === "create" && !failure,
      cancelled: false,
      failure,
      rowsSkipped: 0,
      skipped: [],
      notes: [],
      durationMs: Math.max(1, Math.round(performance.now() - started)),
    } satisfies CsvImportReport;
  } finally {
    running.delete(r.importId);
    cancelled.delete(r.importId);
  }
});

// --- JSON: the same generated rows, as objects (`.ndjson` / `.jsonl`: one per line) -----------

/** A JSON value's text as the backend previews it: strings unquoted, null for empty. */
function jsonCell(raw: string, ty: ImportType): string | null {
  const v = cell(raw);
  if (v === null) return null;
  if (ty === "boolean") return v === "true" ? "true" : "false";
  return v;
}

registerMock("preview_json_import", async ({ req }) => {
  const r = req as JsonPreviewRequest;
  const file = csvFile(r.path);
  const name = baseName(r.path);
  await sleep(mockLatency === 0 ? 0 : 120);
  if (name.toLowerCase().includes("notjson"))
    mockError(
      "INVALID_INPUT",
      "item 1 (line 1) is a number, not an object: the file must hold an array of objects or one object per line",
    );
  return {
    path: r.path,
    fileName: name,
    size: file.rows.length * 120,
    encoding: r.encoding ?? "utf8",
    layout: /\.(ndjson|jsonl)$/i.test(name) ? "lines" : "array",
    keys: file.header,
    columns: file.header.map((key, i) => ({
      name: key,
      detected: column(key, file.types[i] ?? "text"),
    })),
    rows: file.rows
      .slice(0, 100)
      .map((row) => row.map((v, i) => jsonCell(v, file.types[i] ?? "text"))),
    sampledRows: Math.min(1000, file.rows.length),
    more: file.rows.length > 100,
  } satisfies JsonPreview;
});

registerMock("import_json", async ({ connectionId, req, onProgress }) => {
  const r = req as JsonImportRequest;
  if (r.keys.length === 0) mockError("INVALID_INPUT", "the JSON file has no keys to import");
  // The objects go through the CSV wizard's machinery in the backend too.
  return mockInvoke<CsvImportReport>("import_csv", {
    connectionId,
    onProgress,
    req: {
      importId: r.importId,
      path: r.path,
      format: { delimiter: ",", quote: '"', encoding: r.encoding, header: true },
      target: r.target,
      mapping: r.mapping,
      emptyAsNull: true,
      mode: r.mode,
    } satisfies CsvImportRequest,
  });
});

/** A dump "file" with a few bytes that are not UTF-8 (decoded as Windows-1252). */
const latinDump = (path: string) => baseName(path).toLowerCase().includes("latin");

/** The statements of a generated dump. */
function dumpStatements(path: string): string[] {
  const name = baseName(path).toLowerCase();
  if (!path.startsWith("/")) mockError("INVALID_INPUT", "the file path must be absolute");
  const out = [
    "SET NAMES utf8mb4",
    "DROP TABLE IF EXISTS `demo_items`",
    "CREATE TABLE `demo_items` (\n  `id` int NOT NULL,\n  `name` varchar(80) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n)",
  ];
  for (let i = 0; i < 60; i++) {
    out.push(
      `INSERT INTO \`demo_items\` VALUES ${Array.from({ length: 5 }, (_, j) => `(${i * 5 + j}, 'Item ${i * 5 + j}')`).join(", ")}`,
    );
  }
  if (name.includes("bad")) out.splice(10, 0, "INSERT INTO `missing_table` VALUES (1)");
  return out;
}

registerMock("preview_sql_dump", async ({ req }) => {
  const { path } = req as { path: string };
  const statements = dumpStatements(path);
  await sleep(mockLatency === 0 ? 0 : 100);
  return {
    path,
    fileName: baseName(path),
    size: statements.join(";\n").length / (path.endsWith(".gz") ? 6 : 1),
    compressed: path.endsWith(".gz"),
    encoding: latinDump(path) ? "windows1252" : "utf8",
    statements: statements.slice(0, 20),
    more: statements.length > 20,
    invalidUtf8: latinDump(path) ? 3 : 0,
    firstInvalidLine: latinDump(path) ? 12 : null,
  } satisfies SqlDumpPreview;
});

registerMock("import_sql_dump", async ({ req, onProgress }) => {
  const r = req as SqlImportRequest;
  const channel = onProgress as { send?: (p: SqlImportProgress) => void } | undefined;
  if (running.has(r.importId))
    mockError("INVALID_INPUT", `import "${r.importId}" is already running`);
  const statements = dumpStatements(r.path);
  const totalBytes = statements.join(";\n").length;
  const report: SqlImportReport = {
    executed: 0,
    failed: 0,
    skipped: 0,
    errors: [],
    stopped: false,
    cancelled: false,
    encoding: latinDump(r.path) ? "windows1252" : "utf8",
    invalidUtf8: latinDump(r.path) ? 3 : 0,
    firstInvalidLine: latinDump(r.path) ? 12 : null,
    rolledBack: null,
    notes: latinDump(r.path)
      ? ["3 byte sequences were not UTF-8 (the first on line 12): decoded as Windows-1252."]
      : [],
    durationMs: 0,
  };
  const started = performance.now();
  running.add(r.importId);
  try {
    for (const [index, sql] of statements.entries()) {
      if (index % 8 === 0) await sleep(mockLatency === 0 ? 0 : 40);
      if (cancelled.has(r.importId)) {
        report.cancelled = true;
        break;
      }
      if (sql.includes("missing_table")) {
        report.failed++;
        report.errors.push({
          index,
          line: index * 2 + 1,
          sql,
          message: "Table 'tabler_dev.missing_table' doesn't exist",
        });
        if (r.stopOnError) {
          report.stopped = true;
          break;
        }
      } else {
        report.executed++;
      }
      channel?.send?.({
        statements: report.executed + report.failed,
        failed: report.failed,
        bytes: Math.round((totalBytes * (index + 1)) / statements.length),
        totalBytes,
      });
    }
    report.durationMs = Math.max(1, Math.round(performance.now() - started));
    return report;
  } finally {
    running.delete(r.importId);
    cancelled.delete(r.importId);
  }
});

registerMock("cancel_import", ({ importId }) => {
  if (running.has(importId as string)) cancelled.add(importId as string);
  return null;
});
