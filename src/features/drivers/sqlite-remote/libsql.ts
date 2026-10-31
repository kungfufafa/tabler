import type { ConnectionFormValues } from "@/features/connections/schema";
import type { FormIssue, KindUi } from "../types";
import { LibsqlFields } from "./LibsqlFields";
import { libsqlParams, libsqlParamsOf } from "./params";
import { formatLibsqlUrl, libsqlBase, parseLibsqlConnectionUrl, parseLibsqlUrl } from "./url";

/**
 * LibSQL's form (Turso, `sqld`): host, port and SSL as for any server (an SSH tunnel works), or
 * the server's URL for one under a path; the auth token as the secret, or HTTP Basic with a user.
 * HTTPS starts on checking the certificate: the backend sends the token only to the server the
 * name says, and plain "HTTPS" is the choice of a server whose certificate cannot be checked.
 */

/**
 * A URL that does not parse, shown only when nothing in it could be a user, a password or a query
 * token: an unencoded `?`, `#` or `@` inside a password leaves no safe place to cut it.
 */
export const withoutSecrets = (url: string): string => (/[@?#]/.test(url) ? "Invalid URL" : url);

/** What the backend's `base_from_parts` refuses in a host. */
const NOT_A_HOST = /[/?#@\s]/;

function validate(v: ConnectionFormValues): FormIssue[] {
  const { url, basicAuth } = libsqlParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  if (url !== "") {
    const parsed = parseLibsqlUrl(url);
    if (!parsed)
      out.push({
        path: "driverParams.url",
        message: "Use a libsql://, https://, http://, wss:// or ws:// URL",
      });
    else if (parsed.hasSecret)
      out.push({
        path: "driverParams.url",
        message: "Put the token in Auth Token (and the user in User), not in the URL",
      });
    // The tunnel forwards to the profile's host and port, which a URL does not set.
    else if (v.sshEnabled)
      out.push({
        path: "driverParams.url",
        message: "Over SSH, enter the host and port instead of a URL",
      });
  } else if (v.host === "") {
    out.push({ path: "host", message: "Enter the host, or the server's URL" });
  } else if (NOT_A_HOST.test(v.host)) {
    out.push({ path: "host", message: `“${v.host}” is not a host name: paste a URL in URL` });
  }
  if (basicAuth && v.user === "")
    out.push({ path: "user", message: "Basic authentication needs a user" });
  return out;
}

export const libsqlUi: KindUi = {
  rows: ["hostPort", "secret", "ssl"],
  defaults: () => ({ host: "", port: "", ssl: "verify-full" }),
  Fields: LibsqlFields,
  validate,
  toProfile: (v, base) => {
    const { url, basicAuth } = libsqlParamsOf(v.driverParams);
    const params = { ...(url !== "" ? { url } : {}), ...(basicAuth ? { basicAuth: true } : {}) };
    return {
      ...base,
      // One database per server: the sidebar names it after the host.
      database: null,
      user: basicAuth ? base.user : null,
      options: {
        ...base.options,
        driverParams: Object.keys(params).length > 0 ? params : null,
      },
    };
  },
  fromProfile: (p) => ({ driverParams: { ...libsqlParams(p) } }),
  url: {
    schemes: ["libsql"],
    parse: parseLibsqlConnectionUrl,
    format: formatLibsqlUrl,
  },
  summary: (p) => {
    const { url } = libsqlParams(p);
    if (url !== "") {
      // Scheme, host, port and path only: a profile stored before its URL was checked, or one
      // with a token the checks do not know (after `;`, in the fragment), never shows it.
      const parsed = parseLibsqlUrl(url);
      return parsed === null ? withoutSecrets(url) : libsqlBase(parsed);
    }
    return [p.host, p.port === null ? null : String(p.port)].filter(Boolean).join(" : ");
  },
};
