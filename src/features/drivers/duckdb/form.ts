import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { DuckdbFields } from "./DuckdbFields";
import { LOGGER_REFUSED, switchesLogger } from "./logger";
import { duckdbParams, duckdbParamsOf, isMemory, MEMORY_READ_ONLY, toDuckdbParams } from "./params";
import { formatDuckdbUrl, parseDuckdbUrl } from "./url";

/**
 * DuckDB's form, as TablePlus has it: the database file (Choose…, and New… among the lane's
 * fields), or `:memory:` for a database that lives as long as the connection, and the encryption
 * key of an encrypted file in the password field (the file then opens read-only: DuckDB's
 * built-in crypto only decrypts). Read-only opens the file with `access_mode=READ_ONLY` and
 * reads no other file. No host, TLS or SSH: the descriptor offers none.
 */

/** What `entry::validate` refuses, on the fields it names. */
function validate(v: ConnectionFormValues): FormIssue[] {
  const out: FormIssue[] = [];
  if (v.readOnly && isMemory(v.sqlitePath))
    out.push({ path: "readOnly", message: MEMORY_READ_ONLY });
  if (switchesLogger(v.startupSql))
    out.push({ path: "startupSql", message: `Startup SQL: ${LOGGER_REFUSED}` });
  return out;
}

function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  return {
    ...base,
    options: { ...base.options, driverParams: toDuckdbParams(duckdbParamsOf(v.driverParams)) },
  };
}

/** The file, or that it lives in memory. */
function summary(p: ConnectionProfile): string {
  const where = isMemory(p.sqlitePath) ? "In memory" : (p.sqlitePath ?? "").trim();
  return duckdbParams(p).autoloadExtensions ? `${where} (autoload extensions)` : where;
}

export const duckdbUi: KindUi = {
  rows: ["file", "secret"],
  defaults: () => ({ driverParams: { ...duckdbParamsOf(null) } }),
  Fields: DuckdbFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...duckdbParams(p) } }),
  url: {
    schemes: ["duckdb"],
    parse: parseDuckdbUrl,
    format: formatDuckdbUrl,
  },
  summary,
};
