import { formToProfileInput, type ConnectionFormValues } from "@/features/connections/schema";
import type { PasswordMode, ProfileInput } from "@/lib/ipc";
import type { KindFieldsProps } from "../types";
import { bigqueryParamsOf, toBigqueryParams } from "./params";

/**
 * What the form's lists (`listProjects`, `listDatasets`, both reads) send, and how the secret's
 * storage follows the sign-in: only an access token is a secret.
 */

/** One dataset of the project, as `listDatasets` answers it. */
export interface BigqueryDataset {
  name: string;
  location: string | null;
}

/** One project the credentials see, as `listProjects` answers it. */
export interface BigqueryProject {
  id: string;
  name: string | null;
}

/** No saved profile's id reaches the form: a stored token is not used for the lists. */
const UNSAVED = "00000000-0000-0000-0000-000000000000";

/** An access token is never "no password". */
export const TOKEN_MODES: readonly PasswordMode[] = ["keychain", "ask", "command"];

/**
 * The profile and secret the form would connect with, for the form's calls (`listDatasets`,
 * `listProjects`, both reads): the key file or ADC as typed, or the token typed or printed by
 * the command.
 */
export function pickerInput(v: ConnectionFormValues): ProfileInput {
  const input = formToProfileInput(v, null, UNSAVED);
  const params = bigqueryParamsOf(v.driverParams);
  const token = params.auth === "accessToken";
  input.profile = {
    ...input.profile,
    host: null,
    port: null,
    user: null,
    ssh: null,
    database: v.database.trim() || null,
    options: {
      ...input.profile.options,
      passwordMode: token ? v.passwordMode : "none",
      passwordCommand: token && v.passwordMode === "command" ? v.passwordCommand.trim() : null,
      driverParams: { ...toBigqueryParams(params) },
    },
  };
  // A typed token goes with the call whatever its storage; nothing else is a secret here.
  input.password = token && v.password !== "" ? v.password : null;
  return input;
}

/** The secret's storage after the sign-in became `auth`: a token is the secret, the rest read none. */
export function followAuth(form: KindFieldsProps["form"], auth: string): void {
  const mode = form.getValues("passwordMode");
  if (auth === "accessToken" && mode === "none")
    form.setValue("passwordMode", "keychain", { shouldDirty: true });
  else if (auth !== "accessToken" && mode !== "none") {
    form.setValue("passwordMode", "none", { shouldDirty: true });
    form.setValue("password", "");
  }
}

export type Listing<T> =
  | { state: "idle" | "loading" }
  | { state: "done"; items: T[] }
  | { state: "failed"; message: string };

/** What the lists need before they can be asked for; null when they can. */
export function missingForList(v: ConnectionFormValues, hasStoredPassword: boolean): string | null {
  const p = bigqueryParamsOf(v.driverParams);
  if (p.auth === "serviceAccount" && p.keyFilePath === "") return "Choose the key file first";
  if (p.auth === "accessToken" && v.passwordMode !== "command" && v.password === "")
    return hasStoredPassword
      ? "Type the access token again to list (the stored one is used to connect)"
      : "Enter the access token first";
  return null;
}
