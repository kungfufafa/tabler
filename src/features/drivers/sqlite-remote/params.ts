import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";

/**
 * The lane's driver params (`LibsqlParams`, `D1Params` in `src/lib/ipc/bindings`) read leniently
 * from a profile or the form: anything of the wrong type counts as unset, as the backend's
 * `#[serde(default)]` would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

const str = (params: Params, key: string): string => {
  const v = params?.[key];
  return typeof v === "string" ? v.trim() : "";
};

/** LibSQL: the URL that replaces host and port ("" when unset), and Basic authentication. */
export interface LibsqlForm {
  url: string;
  basicAuth: boolean;
}

export function libsqlParamsOf(params: Params): LibsqlForm {
  return { url: str(params, "url"), basicAuth: params?.basicAuth === true };
}

export const libsqlParams = (p: ConnectionProfile): LibsqlForm =>
  libsqlParamsOf(p.options.driverParams);

/** D1: the account ID and the API base ("" when unset: Cloudflare's). */
export interface D1Form {
  accountId: string;
  apiBase: string;
}

export function d1ParamsOf(params: Params): D1Form {
  return { accountId: str(params, "accountId"), apiBase: str(params, "apiBase") };
}

export const d1Params = (p: ConnectionProfile): D1Form => d1ParamsOf(p.options.driverParams);

/** D1's profile: no host, port, user, SSL or SSH, and only the params the backend reads. */
export function d1ToProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const { accountId, apiBase } = d1ParamsOf(v.driverParams);
  return {
    ...base,
    host: null,
    port: null,
    user: null,
    sslCaPath: null,
    options: {
      ...base.options,
      sslKeyPath: null,
      sslCertPath: null,
      driverParams: { accountId, ...(apiBase !== "" ? { apiBase } : {}) },
    },
  };
}
