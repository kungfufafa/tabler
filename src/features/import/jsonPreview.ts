import type { CsvPreview, JsonLayout, JsonPreview } from "@/lib/ipc";

/** The wizard's preview of a JSON file: its keys as the columns, its objects as the rows. */
export interface WizardPreview extends CsvPreview {
  /** JSON: the keys read from each object, in column order (what the import request names). */
  keys: string[] | null;
  layout: JsonLayout | null;
}

export function fromCsvPreview(p: CsvPreview): WizardPreview {
  return { ...p, keys: null, layout: null };
}

/** A JSON preview in the CSV wizard's shape (no delimiter; every key is a named column). */
export function fromJsonPreview(p: JsonPreview): WizardPreview {
  return {
    path: p.path,
    fileName: p.fileName,
    size: p.size,
    delimiter: "",
    encoding: p.encoding,
    columns: p.columns,
    rows: p.rows,
    sampledRows: p.sampledRows,
    more: p.more,
    keys: p.keys,
    layout: p.layout,
  };
}

/** "Array of objects" / "One object per line", for the wizard's header. */
export function layoutLabel(layout: JsonLayout | null): string | null {
  if (layout === "array") return "array of objects";
  if (layout === "lines") return "one object per line";
  return null;
}
