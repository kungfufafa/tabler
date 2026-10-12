import type { Dispatch, ReactNode, SetStateAction } from "react";
import { Checkbox } from "@/components/ui/checkbox";
import { Segmented } from "@/features/settings/controls";
import { cn } from "@/lib/utils";
import {
  CSV_DECIMALS,
  CSV_DELIMITERS,
  CSV_LINE_BREAKS,
  CSV_QUOTES,
  type ExportOptions,
} from "./options";

/** Label column + control column; `id` labels a Segmented group (`${id}-label`). */
export function Row({
  id,
  label,
  hint,
  children,
}: {
  id: string;
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[84px_minmax(0,1fr)] items-start gap-x-3">
      <label
        id={`${id}-label`}
        htmlFor={id}
        className="text-muted-foreground pt-1.5 text-xs leading-4"
      >
        {label}
      </label>
      <div className="flex min-w-0 flex-col items-start gap-1">
        {children}
        {hint && <div className="text-muted-foreground text-[11px] leading-4">{hint}</div>}
      </div>
    </div>
  );
}

export function Check({
  checked,
  onChange,
  disabled,
  title,
  children,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  title?: string;
  children: ReactNode;
}) {
  return (
    <label
      title={title}
      className={cn(
        "flex h-7 cursor-default items-center gap-2 text-xs",
        disabled && "text-muted-foreground opacity-60",
      )}
    >
      <Checkbox
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onChange(v === true)}
      />
      {children}
    </label>
  );
}

const mono = (label: string) => <span className="font-mono">{label}</span>;

/**
 * The options of the chosen format, as TablePlus labels them. `tableSource`: the export reads
 * whole tables, so a SQL script can carry their structure. `sqlExtra` adds rows to the SQL
 * options (the single export's table name and batch size).
 */
export function FormatOptions({
  options,
  setOptions,
  tableSource,
  sqlExtra,
}: {
  options: ExportOptions;
  setOptions: Dispatch<SetStateAction<ExportOptions>>;
  tableSource: boolean;
  sqlExtra?: ReactNode;
}) {
  const set = <K extends "csv" | "json" | "sql" | "xlsx">(
    key: K,
    patch: Partial<ExportOptions[K]>,
  ) => setOptions((o) => ({ ...o, [key]: { ...o[key], ...patch } }));
  const structureTitle = tableSource ? undefined : "Only table exports carry a structure";

  switch (options.format) {
    case "csv":
      return (
        <>
          <Row id="export-csv-options" label="Options">
            <div className="flex flex-col">
              <Check
                checked={options.csv.nullToEmpty}
                onChange={(nullToEmpty) => set("csv", { nullToEmpty })}
              >
                Convert NULL to EMPTY
              </Check>
              <Check
                checked={options.csv.lineBreakToSpace}
                onChange={(lineBreakToSpace) => set("csv", { lineBreakToSpace })}
              >
                Convert line break to space
              </Check>
              <Check checked={options.csv.header} onChange={(header) => set("csv", { header })}>
                Put field names in the first row
              </Check>
            </div>
          </Row>
          <Row id="export-delimiter" label="Delimiter">
            <Segmented
              id="export-delimiter"
              value={options.csv.delimiter}
              onChange={(delimiter) => set("csv", { delimiter })}
              options={CSV_DELIMITERS.map((d) => ({
                ...d,
                label: d.label === "Tab" ? d.label : mono(d.label),
              }))}
            />
          </Row>
          <Row id="export-quote" label="Quote">
            <Segmented
              id="export-quote"
              value={options.csv.quote}
              onChange={(quote) => set("csv", { quote })}
              options={CSV_QUOTES.map((q) => ({
                ...q,
                label: q.value === "ifNeeded" ? q.label : mono(q.label),
              }))}
            />
          </Row>
          <Row id="export-line-break" label="Line break">
            <Segmented
              id="export-line-break"
              value={options.csv.lineBreak}
              onChange={(lineBreak) => set("csv", { lineBreak })}
              options={CSV_LINE_BREAKS.map((l) => ({ ...l, label: mono(l.label) }))}
            />
          </Row>
          <Row id="export-decimal" label="Decimal">
            <Segmented
              id="export-decimal"
              value={options.csv.decimal}
              onChange={(decimal) => set("csv", { decimal })}
              options={CSV_DECIMALS.map((d) => ({ ...d, label: mono(d.label) }))}
            />
          </Row>
        </>
      );
    case "json":
      return (
        <>
          <Row
            id="export-json-layout"
            label="Layout"
            hint={
              options.json.ndjson
                ? "One JSON object per line (NDJSON)"
                : "A single array of row objects"
            }
          >
            <Segmented<"array" | "ndjson">
              id="export-json-layout"
              value={options.json.ndjson ? "ndjson" : "array"}
              onChange={(v) => set("json", { ndjson: v === "ndjson" })}
              options={[
                { value: "array", label: "Array" },
                { value: "ndjson", label: "NDJSON" },
              ]}
            />
          </Row>
          <Row id="export-json-options" label="Options">
            <Check
              checked={options.json.pretty && !options.json.ndjson}
              disabled={options.json.ndjson}
              onChange={(pretty) => set("json", { pretty })}
            >
              Pretty print
            </Check>
          </Row>
        </>
      );
    case "sqlInsert":
      return (
        <>
          <Row
            id="export-sql-options"
            label="Options"
            hint="Not every database feature is exported. Use File ▸ Backup… for full compatibility."
          >
            <div className="flex flex-col">
              <Check
                checked={tableSource && options.sql.dropTable}
                disabled={!tableSource}
                title={structureTitle}
                onChange={(dropTable) => set("sql", { dropTable })}
              >
                Drop table if exists
              </Check>
              <Check
                checked={tableSource && options.sql.structure}
                disabled={!tableSource}
                title={structureTitle}
                onChange={(structure) => set("sql", { structure })}
              >
                Include table structure
              </Check>
              <Check checked={options.sql.content} onChange={(content) => set("sql", { content })}>
                Include table content
              </Check>
              <Check checked={options.sql.gzip} onChange={(gzip) => set("sql", { gzip })}>
                Compress the file using Gzip
              </Check>
            </div>
          </Row>
          {sqlExtra}
        </>
      );
    case "xlsx":
      return (
        <Row id="export-xlsx-options" label="Options">
          <Check checked={options.xlsx.header} onChange={(header) => set("xlsx", { header })}>
            Put field names in the first row
          </Check>
        </Row>
      );
  }
}
