import type { Dialect } from "@/lib/ipc";
import { quoteIdent } from "@/lib/sql/quote";

/** What "New ▸ View… / Function… …" opens a SQL tab with (TablePlus: a raw SQL editor). */
export type NewObjectKind = "view" | "materializedView" | "function" | "procedure";

export interface ObjectTemplate {
  /** Tab title. */
  title: string;
  sql: string;
  /** The placeholder name, selected when the tab opens so typing replaces it. */
  from: number;
  to: number;
}

export const NEW_OBJECT_LABEL: Record<NewObjectKind, string> = {
  view: "View",
  materializedView: "Materialized View",
  function: "Function",
  procedure: "Procedure",
};

/** Which kinds a dialect can create: materialized views are PG-only, routines PG / MySQL. */
export function supportsNewObject(dialect: Dialect, kind: NewObjectKind): boolean {
  switch (kind) {
    case "view":
      return true;
    case "materializedView":
      return dialect === "postgres";
    case "function":
    case "procedure":
      return dialect !== "sqlite";
  }
}

const PLACEHOLDER: Record<NewObjectKind, string> = {
  view: "new_view",
  materializedView: "new_materialized_view",
  function: "new_function",
  procedure: "new_procedure",
};

/** `"schema".` in front of the placeholder: PG always, SQLite for attached databases. */
function prefix(dialect: Dialect, schema: string | null): string {
  if (!schema) return "";
  if (dialect === "postgres") return `${quoteIdent(dialect, schema)}.`;
  if (dialect === "sqlite" && schema !== "main") return `${quoteIdent(dialect, schema)}.`;
  // MySQL: the schema is the connection's current database.
  return "";
}

function body(dialect: Dialect, kind: NewObjectKind): [before: string, after: string] {
  if (dialect === "postgres") {
    switch (kind) {
      case "view":
        return ["CREATE VIEW ", " AS\nSELECT\n  1 AS id;\n"];
      case "materializedView":
        return ["CREATE MATERIALIZED VIEW ", " AS\nSELECT\n  1 AS id\nWITH DATA;\n"];
      case "function":
        return [
          "CREATE OR REPLACE FUNCTION ",
          "(arg integer)\nRETURNS integer\nLANGUAGE plpgsql\nAS $$\nBEGIN\n  RETURN arg;\nEND;\n$$;\n",
        ];
      case "procedure":
        return [
          "CREATE OR REPLACE PROCEDURE ",
          "()\nLANGUAGE plpgsql\nAS $$\nBEGIN\n  -- statements\nEND;\n$$;\n",
        ];
    }
  }
  if (dialect === "mysql") {
    switch (kind) {
      case "function":
        return [
          "CREATE FUNCTION ",
          "(arg INT)\nRETURNS INT\nDETERMINISTIC\nBEGIN\n  RETURN arg;\nEND;\n",
        ];
      case "procedure":
        return ["CREATE PROCEDURE ", "()\nBEGIN\n  SELECT 1;\nEND;\n"];
      default:
        return ["CREATE VIEW ", " AS\nSELECT\n  1 AS id;\n"];
    }
  }
  return ["CREATE VIEW ", " AS\nSELECT\n  1 AS id;\n"];
}

/** The SQL a new view / function / procedure starts from, with its placeholder name located. */
export function newObjectTemplate(
  dialect: Dialect,
  kind: NewObjectKind,
  schema: string | null,
): ObjectTemplate {
  const [before, after] = body(dialect, kind);
  const head = `${before}${prefix(dialect, schema)}`;
  const name = PLACEHOLDER[kind];
  return {
    title: `New ${NEW_OBJECT_LABEL[kind]}`,
    sql: `${head}${name}${after}`,
    from: head.length,
    to: head.length + name.length,
  };
}
