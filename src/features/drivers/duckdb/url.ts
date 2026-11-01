import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile } from "@/lib/ipc";
import { duckdbParams, isMemory, MEMORY } from "./params";

/**
 * DuckDB connection URLs: `duckdb://` and the file's path (TablePlus), as `duckdb:///Users/me/
 * x.duckdb`, `duckdb:///C:/data/x.duckdb` or `duckdb://~/x.duckdb`, and `duckdb:///:memory:`
 * (or `duckdb://` alone, SQLAlchemy's) for an in-memory database. MotherDuck's `md:` and relative
 * paths are no URL. Keys read: `access_mode=read_only` (DuckDB's own setting) or
 * `read_only=true`, `autoload_known_extensions` (DuckDB's) or `autoload`, and an
 * `encryption_key`, which goes to the secret, never into a field. Writing uses DuckDB's keys and
 * never the key.
 */

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** `true` / `yes` / `on` / `1`, `false` / `no` / `off` / `0`; null for anything else. */
function bool(v: string | undefined): boolean | null {
  const t = v?.trim().toLowerCase();
  if (t === undefined) return null;
  if (["true", "yes", "on", "1"].includes(t)) return true;
  if (["false", "no", "off", "0"].includes(t)) return false;
  return null;
}

/** The query's keys in lower case, the last one winning. */
function queryOf(search: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of new URLSearchParams(search)) out.set(k.toLowerCase(), v);
  return out;
}

/** A Windows path: a drive's (`C:/x`, `C:\x`) or a share's (`\\server\share\x`). */
const WINDOWS = /^(?:[A-Za-z]:[\\/]|\\\\)/;

/** The URL's file: the path after `duckdb:` and its slashes; null when it is none. */
function pathOf(raw: string): string | null {
  const rest = decode(raw);
  // `duckdb://<path>`: the authority's two slashes go; `///abs` keeps the root's.
  let path = rest.startsWith("//") ? rest.slice(2) : rest;
  if (path === "" || path === "/") return MEMORY;
  if (path.toLowerCase() === MEMORY || path.toLowerCase() === `/${MEMORY}`) return MEMORY;
  // `duckdb:///C:/x`: the root's slash stands before a Windows path.
  if (path.startsWith("/") && WINDOWS.test(path.slice(1))) path = path.slice(1);
  if (path.startsWith("/") || path.startsWith("~/") || WINDOWS.test(path)) return path;
  return null;
}

/** A `duckdb:` URL as form values; null when it is not one. */
export function parseDuckdbUrl(input: string): ParsedConnectionUrl | null {
  const text = input.trim();
  const m = /^duckdb:([^?#]*)(?:\?([^#]*))?/i.exec(text);
  if (!m) return null;
  const path = pathOf(m[1]!);
  if (path === null) return null;
  const q = queryOf(m[2] ?? "");
  const mode = q.get("access_mode")?.trim().toLowerCase();
  if (mode !== undefined && !["read_only", "read_write", "automatic"].includes(mode)) return null;
  const readOnly = mode === "read_only" || bool(q.get("read_only") ?? q.get("readonly")) === true;
  const autoload = bool(q.get("autoload_known_extensions") ?? q.get("autoload")) === true;
  const values: Partial<ConnectionFormValues> = {
    sqlitePath: path,
    readOnly,
    driverParams: { autoloadExtensions: autoload },
  };
  const name = path === MEMORY ? "" : (path.split(/[\\/]/).filter(Boolean).at(-1) ?? "");
  if (name !== "") values.name = name;
  const key = q.get("encryption_key");
  return { values, password: key ? key : null };
}

/** A path as the URL writes it: what would end the path (`?`, `#`, `%`) escaped. */
const encodePath = (path: string) => encodeURI(path).replaceAll("?", "%3F").replaceAll("#", "%23");

/** The `duckdb://` URL of a profile: the file, read-only and autoload; never the key. */
export function formatDuckdbUrl(p: ConnectionProfile): string | null {
  const file = (p.sqlitePath ?? "").trim();
  const path = isMemory(file) ? MEMORY : file;
  const query: [string, string][] = [];
  if (p.options.readOnly) query.push(["access_mode", "read_only"]);
  if (duckdbParams(p).autoloadExtensions) query.push(["autoload_known_extensions", "true"]);
  const search = query.length > 0 ? `?${new URLSearchParams(query).toString()}` : "";
  // `///abs` and `//~/x`; a slash before a Windows path and `:memory:`.
  const root = path.startsWith("/") || path.startsWith("~/") ? "" : "/";
  return `duckdb://${root}${encodePath(path)}${search}`;
}
