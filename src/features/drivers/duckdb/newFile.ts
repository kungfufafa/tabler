import type { UseFormReturn } from "react-hook-form";
import { toast } from "sonner";
import { formToProfileInput, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc, isMock, toIpcError, type ProfileInput } from "@/lib/ipc";
import { CREATE_FILE, duckdbParamsOf, toDuckdbParams } from "./params";

/**
 * "New…" of DuckDB's form: the save panel picks where, then the backend's `createFile` util
 * writes an empty database there (refusing an existing path, so nothing is ever overwritten).
 * No session exists yet, so there is no connection for Safe Mode to confirm against: the save
 * panel is the confirmation, as for SQLite's New…. A read-only profile refuses the call (it
 * writes), and so does an encryption key (DuckDB in Tabler cannot write encrypted files).
 */

/** No saved profile's id reaches the call: a stored key is not used for it. */
const UNSAVED = "00000000-0000-0000-0000-000000000000";

export const DUCKDB_FILTERS = [{ name: "DuckDB", extensions: ["duckdb", "ddb"] }];

/** Asks where to create the file (the save panel; a prompt in the browser mock). */
export async function pickNewDuckdbFile(): Promise<string | null> {
  if (isMock) return window.prompt("Create DuckDB file at", "/Users/me/database.duckdb");
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title: "New DuckDB database",
    defaultPath: "database.duckdb",
    filters: DUCKDB_FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/** What `createFile` is sent: the form's profile, with the key typed (none otherwise). */
export function createFileInput(v: ConnectionFormValues): ProfileInput {
  const input = formToProfileInput(v, null, UNSAVED);
  const typed = v.password !== "" && v.passwordMode !== "none";
  input.profile = {
    ...input.profile,
    options: {
      ...input.profile.options,
      passwordMode: typed ? "keychain" : "none",
      passwordCommand: null,
      driverParams: toDuckdbParams(duckdbParamsOf(v.driverParams)),
    },
  };
  // A typed key goes with the call, so the backend can refuse it; nothing is stored.
  input.password = typed ? v.password : null;
  input.clearPassword = false;
  return input;
}

/** Creates the file at `path`; resolves to the path the backend wrote. */
export async function createDuckdbFile(v: ConnectionFormValues, path: string): Promise<string> {
  const out = await ipc.driverUtil<{ path: string }>(createFileInput(v), CREATE_FILE, { path });
  return out.path;
}

/** Why New… does nothing on a read-only profile (the backend refuses the call too). */
export const NEW_FILE_READ_ONLY = "Turn off Read-only to create a file";

/**
 * The file row's New… (`KindUi.newFile`): the save panel, then `createFile`; the path the
 * backend wrote goes in the form. A read-only profile and a refusal end in a toast.
 */
export async function newDuckdbFile(form: UseFormReturn<ConnectionFormValues>): Promise<void> {
  const fail = (description: string) =>
    toast.error("Could not create the DuckDB file", { description });
  if (form.getValues("readOnly")) {
    fail(NEW_FILE_READ_ONLY);
    return;
  }
  const path = (await pickNewDuckdbFile())?.trim();
  if (!path) return;
  try {
    const made = await createDuckdbFile(form.getValues(), path);
    form.setValue("sqlitePath", made, { shouldValidate: true, shouldDirty: true });
  } catch (err) {
    fail(toIpcError(err).message);
  }
}
