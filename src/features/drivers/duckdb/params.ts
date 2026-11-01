import type { ConnectionProfile } from "@/lib/ipc";
import type { DuckdbParams } from "@/lib/ipc/bindings/DuckdbParams";

/**
 * The lane's driver params (`DuckdbParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form (anything of the wrong type counts as unset; the backend's
 * `#[serde(default)]` would refuse it with a message of its own), and the database file's rules
 * the form shares with `entry.rs` and `engine::Location`.
 */

type Params = Record<string, unknown> | null | undefined;

/** The form's view of the params. */
export interface DuckdbForm {
  autoloadExtensions: boolean;
}

export const duckdbParamsOf = (params: Params): DuckdbForm => ({
  autoloadExtensions: params?.autoloadExtensions === true,
});

export const duckdbParams = (p: ConnectionProfile): DuckdbForm =>
  duckdbParamsOf(p.options.driverParams);

/** The params a profile keeps: only what differs from the backend's defaults. */
export function toDuckdbParams(f: DuckdbForm): Partial<DuckdbParams> | null {
  return f.autoloadExtensions ? { autoloadExtensions: true } : null;
}

/** `:memory:`, the file name of an in-memory database. */
export const MEMORY = ":memory:";

/** `Location::parse`: nothing or `:memory:` (any case) is an in-memory database. */
export function isMemory(path: string | null | undefined): boolean {
  const p = (path ?? "").trim();
  return p === "" || p.toLowerCase() === MEMORY;
}

/** `entry::validate`'s refusal of a read-only in-memory database. */
export const MEMORY_READ_ONLY =
  "An in-memory DuckDB database cannot be opened read-only: choose a file.";

/** `entry::util`'s refusal of "New…" with an encryption key. */
export const NO_ENCRYPTED_FILES =
  "DuckDB in Tabler cannot create encrypted files: clear the encryption key, or create the file with the DuckDB CLI.";

/** `errors::EXTERNAL_ACCESS_HINT`: why a hardened session reads no file. */
export const EXTERNAL_ACCESS_HINT =
  "This session may not read files or reach other databases: it is read-only, or it runs for MCP or a plugin.";

/** `entry::CHECK_FILE` (reads) and `entry::CREATE_FILE` (New…, writes). */
export const CHECK_FILE = "checkFile";
export const CREATE_FILE = "createFile";
