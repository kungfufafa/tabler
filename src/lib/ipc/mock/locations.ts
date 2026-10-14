// Mock handlers for Settings ▸ Locations: get_locations, inspect_location, set_data_location,
// set_queries_location, reveal_location, restart_app. Mirrors `src-tauri/src/locations.rs`
// (validation, data folder changes applying after a restart, queries switching at once).
import type { LocationChange } from "../bindings/LocationChange";
import type { LocationInspection } from "../bindings/LocationInspection";
import type { LocationKind } from "../bindings/LocationKind";
import type { LocationsInfo } from "../bindings/LocationsInfo";
import { mockError, registerMock } from "./transport";

const DEFAULT_DIR = "/Users/me/Library/Application Support/com.tabler.app";
const DEFAULT_QUERIES = `${DEFAULT_DIR}/Queries`;

/** Mock folders: path → whether it holds an app DB, and how many .sql files. */
const FOLDERS: Record<string, { db: boolean; sql: number }> = {
  [DEFAULT_DIR]: { db: true, sql: 0 },
  [DEFAULT_QUERIES]: { db: false, sql: 5 },
  "/Users/me/Dropbox/Tabler": { db: false, sql: 0 },
  "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Tabler": { db: true, sql: 3 },
  "/Users/me/queries": { db: false, sql: 2 },
};

let info: LocationsInfo;

function initial(): LocationsInfo {
  return {
    dataDir: DEFAULT_DIR,
    defaultDataDir: DEFAULT_DIR,
    configuredDataDir: null,
    dataFile: `${DEFAULT_DIR}/tabler.sqlite`,
    dataFallback: false,
    pendingDataDir: null,
    queriesDir: DEFAULT_QUERIES,
    defaultQueriesDir: DEFAULT_QUERIES,
    configuredQueriesDir: null,
    queriesFallback: false,
    warnings: [],
  };
}
info = initial();

/** Test helper: back to the default folders (optionally with startup warnings). */
export function resetMockLocations(opts: { warnings?: string[] } = {}): void {
  info = { ...initial(), warnings: opts.warnings ?? [] };
}

const norm = (p: string) => p.replace(/\/+$/, "") || "/";

function inspect(kind: LocationKind, path: string): LocationInspection {
  const p = norm(path);
  const folder = FOLDERS[p];
  const current = kind === "data" ? (info.configuredDataDir ?? info.dataDir) : info.queriesDir;
  const out: LocationInspection = {
    path: p,
    isCurrent: p === norm(current),
    hasData: kind === "data" ? (folder?.db ?? false) : (folder?.sql ?? 0) > 0,
    queryFiles: kind === "queries" ? (folder?.sql ?? 0) : 0,
    problem: null,
  };
  if (!p.startsWith("/")) out.problem = "Choose a folder (an absolute path).";
  else if (!folder) out.problem = "The folder does not exist.";
  else if (
    kind === "queries" &&
    !out.isCurrent &&
    (p.startsWith(`${norm(info.queriesDir)}/`) || info.queriesDir.startsWith(`${p}/`))
  )
    out.problem = "The folder is inside the current queries folder, or holds it.";
  return out;
}

registerMock("get_locations", () => structuredClone(info));

registerMock("inspect_location", ({ kind, path }) =>
  inspect(kind as LocationKind, String(path ?? "")),
);

registerMock("set_data_location", ({ path, change }) => {
  const target = norm((path as string | null) ?? DEFAULT_DIR);
  const seen = inspect("data", target);
  if (seen.problem) mockError("INVALID_INPUT", seen.problem);
  const c = change as LocationChange;
  if (target !== info.dataDir) {
    if (c === "move" && seen.hasData)
      mockError("INVALID_INPUT", "The folder already holds Tabler data: use it, or replace it.");
    if (c === "empty" && seen.hasData)
      mockError("INVALID_INPUT", "The folder already holds Tabler data.");
    if (c === "useExisting" && !seen.hasData)
      mockError("INVALID_INPUT", "The folder holds no Tabler data.");
  }
  info = {
    ...info,
    configuredDataDir: target === DEFAULT_DIR ? null : target,
    pendingDataDir: target === info.dataDir ? null : target,
  };
  return structuredClone(info);
});

registerMock("set_queries_location", ({ path, change }) => {
  const target = norm((path as string | null) ?? DEFAULT_QUERIES);
  const seen = inspect("queries", target);
  if (seen.problem) mockError("INVALID_INPUT", seen.problem);
  const from = FOLDERS[norm(info.queriesDir)];
  const to = FOLDERS[target];
  if (!seen.isCurrent && (change === "move" || change === "replace") && from && to) {
    to.sql += from.sql;
    from.sql = 0;
  }
  info = {
    ...info,
    queriesDir: target,
    configuredQueriesDir: target === DEFAULT_QUERIES ? null : target,
    queriesFallback: false,
  };
  return structuredClone(info);
});

registerMock("reveal_location", () => null);

registerMock("restart_app", () => {
  // The browser stand-in for a restart: the new folders are in use.
  if (info.pendingDataDir) {
    info = {
      ...info,
      dataDir: info.pendingDataDir,
      dataFile: `${info.pendingDataDir}/tabler.sqlite`,
      pendingDataDir: null,
    };
  }
  window.location.reload();
  return null;
});
