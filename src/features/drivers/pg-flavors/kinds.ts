import type { ConnectionFormValues } from "@/features/connections/schema";
import type { FormIssue, KindUi, SharedRow } from "../types";
import { RedshiftFields } from "./RedshiftFields";
import { FLAVOR_SCHEMES, type FlavorKind, formatFlavorUrl, parseFlavorUrl } from "./url";

/**
 * The tiles of CockroachDB, Greenplum and Redshift. Their profiles are PostgreSQL ones
 * (`pg_flavor` names the engine, there are no driver params), so the app shows the PostgreSQL
 * form with the tile's port, user and database; the lane adds its defaults, the checks of what
 * an engine cannot take, its URLs and Redshift's IAM row. `rows` and `sslModes` say what that
 * form is, for code that reads them.
 */

const ROWS: SharedRow[] = ["hostPort", "user", "secret", "database", "ssl"];

/** The PostgreSQL SSL modes, the same menu the form shows for every PostgreSQL profile. */
const PG_SSL_MODES = [
  ["disable", "disable"],
  ["prefer", "prefer"],
  ["require", "require"],
  ["verify-ca", "verify-ca"],
  ["verify-full", "verify-full"],
] as const;

const LABEL: Record<FlavorKind, string> = {
  cockroachdb: "CockroachDB",
  greenplum: "Greenplum",
  redshift: "Redshift",
};

/**
 * Direct TLS negotiation is PostgreSQL 17's handshake; Greenplum (PostgreSQL 9.4 to 14) and
 * Redshift (8.0) answer it as a broken startup packet.
 */
function directTls(kind: FlavorKind, v: ConnectionFormValues): FormIssue[] {
  if (kind === "cockroachdb" || v.pgSslNegotiation !== "direct") return [];
  return [
    {
      path: "pgSslNegotiation",
      message: `${LABEL[kind]} has no direct TLS negotiation (PostgreSQL 17's): use Postgres`,
    },
  ];
}

/** Redshift signs in with a password (MD5) or an IAM one; it has no trust authentication. */
function redshiftPassword(v: ConnectionFormValues): FormIssue[] {
  if (v.passwordMode !== "none") return [];
  return [
    {
      path: "passwordMode",
      message:
        "Redshift asks every user for a password: store it, ask for it or run a command (IAM)",
    },
  ];
}

/** `connections/encoding.ts::isUtf8Encoding`'s names, without its store and toast imports. */
const UTF8_ENCODINGS = ["utf8", "utf-8", "utf8mb3", "utf8mb4", "unicode"];

/**
 * Another encoding makes the session read-only, which Redshift has no setting to enforce, so
 * the backend refuses that connect (`imp/redshift.rs::check_init`) unless read-only is on.
 */
function redshiftEncoding(v: ConnectionFormValues): FormIssue[] {
  const encoding = v.encoding.trim().toLowerCase();
  if (v.readOnly || !encoding || UTF8_ENCODINGS.includes(encoding)) return [];
  return [
    {
      path: "encoding",
      message: "Redshift speaks only UTF-8: leave Encoding empty, or turn read-only on",
    },
  ];
}

function kindUi(kind: FlavorKind, extra: Partial<KindUi>): KindUi {
  return {
    rows: ROWS,
    sslModes: PG_SSL_MODES,
    url: {
      schemes: FLAVOR_SCHEMES[kind],
      parse: (url, source) => parseFlavorUrl(kind, url, source),
      format: (p) => formatFlavorUrl(kind, p),
    },
    ...extra,
    validate: (v) => [...directTls(kind, v), ...(extra.validate?.(v) ?? [])],
  };
}

/** The user a CockroachDB node starts with (`root`); the port comes from the tile. */
export const cockroachdbUi = kindUi("cockroachdb", { defaults: () => ({ user: "root" }) });

export const greenplumUi = kindUi("greenplum", {});

/** Redshift takes TLS on every endpoint (Serverless and new clusters require it). */
export const redshiftUi = kindUi("redshift", {
  defaults: () => ({ ssl: "require" }),
  Fields: RedshiftFields,
  validate: (v) => [...redshiftPassword(v), ...redshiftEncoding(v)],
});
