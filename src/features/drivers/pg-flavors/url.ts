import { KIND_DESCRIPTORS } from "@/features/drivers/catalog";
import {
  parseConnectionUrl,
  profileToUrl,
  type ParsedConnectionUrl,
} from "@/features/connections/url";
import type { ConnectionProfile, DatabaseKind } from "@/lib/ipc";

/**
 * Connection URLs of the PostgreSQL flavors: a PostgreSQL URL under the engine's own scheme
 * (`cockroachdb://root@host:26257/defaultdb?sslmode=verify-full`), read and written by the
 * PostgreSQL URL code so every PostgreSQL query parameter means the same here. A URL without a
 * port gets the engine's (26257, 5432, 5439). `postgres://` URLs stay plain PostgreSQL ones: the
 * server's `version()` names the engine at connect anyway.
 */

export type FlavorKind = Extract<DatabaseKind, "cockroachdb" | "greenplum" | "redshift">;

/** Each engine's schemes, the first the one its URLs are written with. */
export const FLAVOR_SCHEMES: Record<FlavorKind, readonly string[]> = {
  cockroachdb: ["cockroachdb", "cockroach", "crdb"],
  greenplum: ["greenplum", "gpdb"],
  redshift: ["redshift"],
};

const SCHEME = /^([a-z][a-z0-9+.-]*):\/\//i;

/** The port a URL names ("" for none); null when it is no URL. */
function urlPort(url: string): string | null {
  try {
    return new URL(url).port;
  } catch {
    return null;
  }
}

/** Reads a URL of `kind` (its SSH part already taken off by the app). */
export function parseFlavorUrl(kind: FlavorKind, url: string): ParsedConnectionUrl | null {
  const scheme = SCHEME.exec(url.trim())?.[1]?.toLowerCase();
  if (!scheme || !FLAVOR_SCHEMES[kind].includes(scheme)) return null;
  const pg = url.trim().replace(SCHEME, "postgresql://");
  let parsed: ParsedConnectionUrl;
  try {
    parsed = parseConnectionUrl(pg);
  } catch {
    return null;
  }
  const explicitPort = urlPort(pg);
  if (explicitPort === null) return null;
  const port = KIND_DESCRIPTORS[kind].defaultPort;
  return {
    ...parsed,
    values: {
      ...parsed.values,
      pgFlavor: KIND_DESCRIPTORS[kind].pgFlavor,
      ...(explicitPort === "" && port !== null ? { port: String(port) } : {}),
    },
  };
}

/**
 * The URL of a profile of `kind`: PostgreSQL's with the engine's scheme. The app adds the SSH
 * server (`<scheme>+ssh://…`), so it is written without; key authentication is kept in the query
 * as the PostgreSQL URL keeps it.
 */
export function formatFlavorUrl(kind: FlavorKind, p: ConnectionProfile): string {
  const plain = profileToUrl({ ...p, ssh: null, options: { ...p.options, pgFlavor: null } });
  const url = plain.replace(SCHEME, `${FLAVOR_SCHEMES[kind][0]}://`);
  if (p.ssh?.auth !== "key") return url;
  return `${url}${url.includes("?") ? (url.endsWith("?") ? "" : "&") : "?"}usePrivateKey=true`;
}
