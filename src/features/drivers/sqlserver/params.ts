import type { ConnectionProfile } from "@/lib/ipc";
import type { MssqlAuth } from "@/lib/ipc/bindings/MssqlAuth";
import type { MssqlParams } from "@/lib/ipc/bindings/MssqlParams";

/**
 * The lane's driver params (`MssqlParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

/** The logins in the order the form offers them, with their labels. */
export const AUTH_METHODS: readonly (readonly [MssqlAuth, string])[] = [
  ["sqlServer", "SQL Server login"],
  ["windows", "Windows (NTLM, DOMAIN\\user)"],
  ["integrated", "Integrated (this Windows user)"],
  ["entraToken", "Microsoft Entra ID token"],
];

const AUTHS = new Set<string>(AUTH_METHODS.map(([a]) => a));

/** `MssqlAuth::needs_user`. */
export const needsUser = (auth: MssqlAuth): boolean => auth === "sqlServer" || auth === "windows";

/** `MssqlAuth::available`: Windows and integrated logins exist in the Windows build only. */
export const windowsOnly = (auth: MssqlAuth): boolean =>
  auth === "windows" || auth === "integrated";

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface MssqlForm {
  auth: MssqlAuth;
  instance: string;
  strict: boolean;
  hostnameInCertificate: string;
  readOnlyIntent: boolean;
  applicationName: string;
  multiSubnetFailover: boolean;
  /** Digits as typed; "" for the server's size. */
  packetSize: string;
}

const str = (params: Params, key: string): string => {
  const v = params?.[key];
  return typeof v === "string" ? v.trim() : "";
};

const flag = (params: Params, key: string): boolean => params?.[key] === true;

export function mssqlParamsOf(params: Params): MssqlForm {
  const auth = params?.auth;
  const size = params?.packetSize;
  return {
    auth: typeof auth === "string" && AUTHS.has(auth) ? (auth as MssqlAuth) : "sqlServer",
    instance: str(params, "instance"),
    strict: flag(params, "strict"),
    hostnameInCertificate: str(params, "hostnameInCertificate"),
    readOnlyIntent: flag(params, "readOnlyIntent"),
    applicationName: str(params, "applicationName"),
    multiSubnetFailover: flag(params, "multiSubnetFailover"),
    packetSize:
      typeof size === "number" && Number.isInteger(size)
        ? String(size)
        : typeof size === "string"
          ? size.trim()
          : "",
  };
}

export const mssqlParams = (p: ConnectionProfile): MssqlForm =>
  mssqlParamsOf(p.options.driverParams);

/**
 * The params a profile keeps: only what differs from the backend's defaults, so a profile with
 * none stores none. The packet size is a number (the form checks its digits first).
 */
export function toMssqlParams(f: MssqlForm): Partial<MssqlParams> | null {
  const out: Partial<MssqlParams> = {};
  if (f.auth !== "sqlServer") out.auth = f.auth;
  if (f.instance !== "") out.instance = f.instance;
  if (f.strict) out.strict = true;
  if (f.hostnameInCertificate !== "") out.hostnameInCertificate = f.hostnameInCertificate;
  if (f.readOnlyIntent) out.readOnlyIntent = true;
  if (f.applicationName !== "") out.applicationName = f.applicationName;
  if (f.multiSubnetFailover) out.multiSubnetFailover = true;
  if (/^\d+$/.test(f.packetSize)) out.packetSize = Number(f.packetSize);
  return Object.keys(out).length > 0 ? out : null;
}

/** `MssqlParams::check`'s instance name: letters, digits, `_`, `$` or `#`, at most 16 UTF-8 bytes
 * (Rust's `str::len`). */
export const INSTANCE_NAME = /^[\p{L}\p{N}_$#]+$/u;
export const instanceNameOk = (name: string): boolean =>
  INSTANCE_NAME.test(name) && new TextEncoder().encode(name).length <= 16;

/** What the form says of the instance: the backend looks it up only without a port or SSH. */
export function instanceHint(sshEnabled: boolean, port: string): string {
  if (sshEnabled)
    return "Ignored over SSH: SQL Browser answers on UDP 1434, which the tunnel does not carry, so give the instance's port";
  if (port.trim() !== "")
    return "Ignored while a port is set: empty the port to find the instance through SQL Browser (UDP 1434)";
  return "Optional: a named instance, found through SQL Browser (UDP 1434) when the port is empty";
}

/** `MssqlParams::check`'s packet size, in bytes. */
export const PACKET_SIZE = { min: 512, max: 32767 } as const;
