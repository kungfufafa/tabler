import type { ConnectionFormValues } from "@/features/connections/schema";
import type { FormIssue, KindUi } from "../types";
import { D1Fields } from "./D1Fields";
import { d1Params, d1ParamsOf, d1ToProfile } from "./params";
import { formatD1Url, parseD1ConnectionUrl } from "./url";

/**
 * Cloudflare D1's form: the account ID (a driver param), the API token as the secret, and the
 * database by name or UUID, which the account's list fills in (`listDatabases`, a read). D1 has
 * no host, port, user, SSL or SSH: the API's certificate is always checked.
 */

/** The backend's `D1Params::api_base` check: the token must not travel in clear text. */
const LOOPBACK = /^(localhost|127(?:\.\d{1,3}){3}|\[::1\])$/i;

const ACCOUNT_ID = /^[0-9a-f]{32}$/i;

export const API_BASE_SECRET =
  "The API base takes no user, password, query or fragment: put the API token in the API Token field";

function apiBaseIssue(base: string): string | null {
  if (base === "") return null;
  let url: URL;
  try {
    url = new URL(base);
  } catch {
    return "Enter the API base as a URL (https://…)";
  }
  // Stored as plain text and written into links; a user and password go out as Basic auth. Any
  // `?` or `#` counts, even an empty one, as the backend's parser sees it.
  if (url.username !== "" || url.password !== "" || /[?#]/.test(base)) return API_BASE_SECRET;
  if (
    url.protocol === "https:" ||
    (url.protocol === "http:" && LOOPBACK.test(url.host.replace(/:\d+$/, "")))
  )
    return null;
  return "The API base must be an https:// URL (http:// only to this machine)";
}

function validate(v: ConnectionFormValues): FormIssue[] {
  const { accountId, apiBase } = d1ParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  if (accountId === "")
    out.push({ path: "driverParams.accountId", message: "Enter the account ID" });
  else if (!ACCOUNT_ID.test(accountId))
    out.push({
      path: "driverParams.accountId",
      message: "The account ID is 32 hexadecimal characters (Workers & Pages ▸ Overview)",
    });
  const base = apiBaseIssue(apiBase);
  if (base) out.push({ path: "driverParams.apiBase", message: base });
  return out;
}

export const d1Ui: KindUi = {
  rows: ["secret"],
  sslModes: null,
  Fields: D1Fields,
  validate,
  toProfile: d1ToProfile,
  fromProfile: (p) => ({ driverParams: { ...d1Params(p) } }),
  url: {
    schemes: ["d1", "cloudflared1"],
    parse: parseD1ConnectionUrl,
    format: formatD1Url,
  },
  summary: (p) => {
    const { accountId } = d1Params(p);
    const account = accountId === "" ? "" : ` · account ${accountId.slice(0, 8)}…`;
    return `${p.database ?? ""}${account}`;
  },
};
