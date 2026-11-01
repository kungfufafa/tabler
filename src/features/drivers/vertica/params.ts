import type { ConnectionProfile } from "@/lib/ipc";
import type { VerticaAuth } from "@/lib/ipc/bindings/VerticaAuth";
import type { VerticaParams } from "@/lib/ipc/bindings/VerticaParams";

/**
 * The lane's driver params (`VerticaParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

/** What the password field holds, in the order the form offers them, with their labels. */
export const AUTH_METHODS: readonly (readonly [VerticaAuth, string])[] = [
  ["password", "Password"],
  ["oauth", "OAuth access token"],
];

const AUTHS = new Set<string>(AUTH_METHODS.map(([a]) => a));

/** `params::MAX_NAME`: the longest client label or workload Tabler sends. */
export const MAX_NAME = 128;

/** `params::DEFAULT_CLIENT_LABEL`: what `v_monitor.sessions` shows without a label. */
export const DEFAULT_CLIENT_LABEL = "Tabler";

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface VerticaForm {
  auth: VerticaAuth;
  clientLabel: string;
  workload: string;
  manualCommit: boolean;
}

const str = (params: Params, key: string): string => {
  const v = params?.[key];
  return typeof v === "string" ? v.trim() : "";
};

export function verticaParamsOf(params: Params): VerticaForm {
  const auth = params?.auth;
  return {
    auth: typeof auth === "string" && AUTHS.has(auth) ? (auth as VerticaAuth) : "password",
    clientLabel: str(params, "clientLabel"),
    workload: str(params, "workload"),
    manualCommit: params?.manualCommit === true,
  };
}

export const verticaParams = (p: ConnectionProfile): VerticaForm =>
  verticaParamsOf(p.options.driverParams);

/** The params a profile keeps: only what differs from the backend's defaults. */
export function toVerticaParams(f: VerticaForm): Partial<VerticaParams> | null {
  const out: Partial<VerticaParams> = {};
  if (f.auth !== "password") out.auth = f.auth;
  if (f.clientLabel !== "") out.clientLabel = f.clientLabel;
  if (f.workload !== "") out.workload = f.workload;
  if (f.manualCommit) out.manualCommit = true;
  return Object.keys(out).length > 0 ? out : null;
}

/**
 * `VerticaParams::check`: a label or workload goes into the startup message, which takes up to
 * 128 characters (Rust's `chars().count()`: code points) and no control characters. Null: fine.
 */
export function nameProblem(value: string): string | null {
  if ([...value].length > MAX_NAME || /\p{Cc}/u.test(value))
    return `Up to ${MAX_NAME} characters, no line breaks`;
  return null;
}
