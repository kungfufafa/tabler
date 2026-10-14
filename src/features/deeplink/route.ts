import type { ConnectionFormValues } from "@/features/connections/schema";
import { parseDeepLink, type UrlFilter, type UrlTarget } from "@/features/connections/url";
import { ANY_COLUMN, OPS, RAW_SQL_COLUMN } from "@/features/table-tab/filters";
import type { ConnectionProfile, FilterOp, OpenRequest } from "@/lib/ipc";

/**
 * Where a URL or file Tabler was opened with goes (TablePlus §1.10, §1.5) — pure, so every case
 * is unit-tested; `open.ts` carries the plans out:
 * - a connection URL that matches a saved connection (driver, host, port, user, database; the
 *   SSH server too) opens that connection; otherwise a temporary connection is opened;
 * - `tabler://connection/<id>` opens that saved connection;
 * - a SQLite file opens its saved connection (same path) or a temporary one;
 * - an `.sql` (or `.sql.gz`) file opens in a query tab of the active connection, like File ▸
 *   Open; without one it asks which connection first.
 */

export type OpenPlan =
  | {
      kind: "profile";
      profileId: string;
      /** The URL's passwords: used for this connect only. */
      password: string | null;
      sshPassword: string | null;
      target: UrlTarget | null;
      isolated: boolean;
      /** The link's `lazyload`: the sidebar setting of that saved connection (null: untouched). */
      lazyLoad: boolean | null;
    }
  | {
      kind: "temporary";
      values: Partial<ConnectionFormValues>;
      password: string | null;
      sshPassword: string | null;
      target: UrlTarget | null;
      isolated: boolean;
      /** The link's `lazyload`: the temporary connection's sidebar lazy loading (null: default). */
      lazyLoad: boolean | null;
    }
  /** `connectionId` null: ask which connection first. */
  | { kind: "sqlFile"; path: string; connectionId: string | null }
  | { kind: "error"; message: string };

export interface RouteContext {
  profiles: readonly ConnectionProfile[];
  /** Profiles opened from a URL and not saved (matched last). */
  temporary: ReadonlySet<string>;
  /** The connection an `.sql` file opens in. */
  activeConnectionId: string | null;
}

const LOOPBACK = new Set(["localhost", "127.0.0.1", "::1"]);

function sameHost(a: string | null | undefined, b: string | null | undefined): boolean {
  const x = (a ?? "").trim().toLowerCase();
  const y = (b ?? "").trim().toLowerCase();
  return x === y || (LOOPBACK.has(x) && LOOPBACK.has(y));
}

const text = (v: string | null | undefined) => (v ?? "").trim();

/** The saved connection a URL describes, if any: a saved one before a temporary one. */
export function findMatchingProfile(
  values: Partial<ConnectionFormValues>,
  profiles: readonly ConnectionProfile[],
  temporary: ReadonlySet<string> = new Set(),
): ConnectionProfile | null {
  const matches = profiles.filter((p) => {
    if (p.driver !== values.driver) return false;
    const ssh = values.sshEnabled === true;
    if (ssh !== (p.ssh !== null)) return false;
    if (ssh && p.ssh) {
      if (!sameHost(p.ssh.host, values.sshHost)) return false;
      if (text(values.sshUser) !== "" && text(p.ssh.user) !== text(values.sshUser)) return false;
    }
    if (p.driver === "sqlite") return text(p.sqlitePath) === text(values.sqlitePath);
    const defaultPort = p.driver === "postgres" ? 5432 : 3306;
    const port = values.port ? Number(values.port) : defaultPort;
    return (
      sameHost(p.host, values.host) &&
      (p.port ?? defaultPort) === port &&
      text(p.user) === text(values.user) &&
      text(p.database) === text(values.database)
    );
  });
  return matches.find((p) => !temporary.has(p.id)) ?? matches[0] ?? null;
}

const lower = (p: string) => p.toLowerCase();

function fileName(path: string): string {
  return path.split("/").filter(Boolean).at(-1) ?? path;
}

/** What to do with one request. */
export function planOpen(request: OpenRequest, ctx: RouteContext): OpenPlan {
  if (request.kind === "file") {
    const path = request.path;
    const name = lower(fileName(path));
    if (name.endsWith(".sql") || name.endsWith(".sql.gz"))
      return { kind: "sqlFile", path, connectionId: ctx.activeConnectionId };
    if (![".db", ".sqlite", ".sqlite3"].some((ext) => name.endsWith(ext)))
      return { kind: "error", message: `Tabler cannot open “${fileName(path)}”.` };
    const values: Partial<ConnectionFormValues> = {
      driver: "sqlite",
      sqlitePath: path,
      name: fileName(path),
    };
    const saved = findMatchingProfile(values, ctx.profiles, ctx.temporary);
    return saved
      ? {
          kind: "profile",
          profileId: saved.id,
          password: null,
          sshPassword: null,
          target: null,
          isolated: false,
          lazyLoad: null,
        }
      : {
          kind: "temporary",
          values,
          password: null,
          sshPassword: null,
          target: null,
          isolated: false,
          lazyLoad: null,
        };
  }
  let link;
  try {
    link = parseDeepLink(request.url);
  } catch (e) {
    return { kind: "error", message: e instanceof Error ? e.message : String(e) };
  }
  if (link.kind === "saved") {
    if (!ctx.profiles.some((p) => p.id === link.profileId))
      return { kind: "error", message: "The link names a connection that is not saved here." };
    return {
      kind: "profile",
      profileId: link.profileId,
      password: null,
      sshPassword: null,
      target: link.target,
      isolated: link.extras.windowMode === "isolated",
      lazyLoad: link.extras.lazyload,
    };
  }
  const { parsed } = link;
  const { sshPassword = null, ...values } = parsed.values;
  const isolated = parsed.extras?.windowMode === "isolated";
  const saved = findMatchingProfile(values, ctx.profiles, ctx.temporary);
  if (saved)
    return {
      kind: "profile",
      profileId: saved.id,
      password: parsed.password,
      sshPassword: sshPassword || null,
      target: parsed.target ?? null,
      isolated,
      lazyLoad: parsed.extras?.lazyload ?? null,
    };
  return {
    kind: "temporary",
    values,
    password: parsed.password,
    sshPassword: sshPassword || null,
    target: parsed.target ?? null,
    isolated,
    lazyLoad: parsed.extras?.lazyload ?? null,
  };
}

const OPERATOR_ALIASES: Record<string, FilterOp> = {
  "=": "eq",
  "==": "eq",
  is: "eq",
  equals: "eq",
  "!=": "ne",
  "<>": "ne",
  "not equal": "ne",
  "<": "lt",
  ">": "gt",
  "<=": "le",
  ">=": "ge",
  "starts with": "hasPrefix",
  "ends with": "hasSuffix",
  null: "isNull",
  "not null": "isNotNull",
};

/** A link's operator (`=`, `>=`, `LIKE`, `Contains`, `IS NULL`, an op id…) as a filter op. */
export function filterOpOf(operator: string): FilterOp | null {
  const o = operator.trim().replace(/\s+/g, " ");
  const key = o.toLowerCase();
  if (key in OPERATOR_ALIASES) return OPERATOR_ALIASES[key]!;
  const byLabel = OPS.find(
    (d) => d.label.toLowerCase() === key || d.label.toLowerCase().replace(/ – /g, " ") === key,
  );
  if (byLabel) return byLabel.op;
  const byId = OPS.find((d) => d.op.toLowerCase() === key.replace(/[\s_-]/g, ""));
  return byId?.op ?? null;
}

/** A filter-bar row (column, op, values; raw rows use RAW_SQL_COLUMN). */
export interface FilterRowSeed {
  column: string;
  op: FilterOp;
  value: string;
  value2: string;
  /**
   * Whether the link's own condition applies right away: column conditions do; a raw SQL
   * condition does not (`false`: it waits for the user, see `open.ts` `navigate`).
   */
  enabled: boolean;
}

/**
 * The filter-bar row of a link's filter. Column conditions are applied (their values are bound
 * parameters). A raw SQL condition (`condition` / `raw` / `query`, TablePlus b570) is SQL of the
 * link's own, which a link from anywhere must not run on a saved connection unasked: its row comes
 * back unchecked (`enabled: false`), and `navigate` applies it once the user confirms in a dialog
 * that shows the SQL, or at once on a read-only connection (which cannot change anything).
 */
export function filterRowOf(filter: UrlFilter): FilterRowSeed | { error: string } {
  if (filter.kind === "raw")
    return { column: RAW_SQL_COLUMN, op: "eq", value: filter.sql, value2: "", enabled: false };
  const op = filterOpOf(filter.operator);
  if (!op) return { error: `Unknown filter operator “${filter.operator}”` };
  let [value, value2] = [filter.value, ""];
  if (op === "between" || op === "notBetween") {
    const parts = filter.value.split(/\s+and\s+|,/i).map((s) => s.trim());
    [value, value2] = [parts[0] ?? "", parts[1] ?? ""];
  }
  // The filter bar's pseudo-columns are reserved: a link naming one of them as a "column" would
  // become a raw SQL row (`RAW_SQL_COLUMN`) that runs unchecked. Only `*` reaches ANY_COLUMN.
  if (filter.column === RAW_SQL_COLUMN || filter.column === ANY_COLUMN)
    return { error: `Unknown column “${filter.column}”` };
  const column = filter.column === "*" ? ANY_COLUMN : filter.column;
  return { column, op, value, value2, enabled: true };
}
