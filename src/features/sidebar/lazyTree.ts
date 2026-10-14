import { isSearching, type MatchStrategy } from "@/features/objects/search";
import type { RoutineSummary, SequenceSummary, TableSummary } from "@/lib/ipc";
import { EMPTY_PINS } from "@/stores/pins";
import { EMPTY_RECENT } from "@/stores/recent";
import { EMPTY_GROUPS, type SidebarGroup } from "./groups";
import { EMPTY_COLLAPSED, type SidebarSection } from "./sidebarPrefs";
import { buildTreeRows, mergeObjectRows, nodePrefix, type TreeRow } from "./useSchemaTree";

/** One schema (PostgreSQL) or database (MySQL) node of the lazy-loaded tree. */
export interface LazyNode {
  schema: string;
  /** PostgreSQL's information_schema / pg_catalog / pg_toast. */
  system: boolean;
  expanded: boolean;
  /** Its tables and views; undefined while they are not loaded. */
  tables: readonly TableSummary[] | undefined;
  /** Functions / procedures and sequences (undefined: section off or not loaded). */
  routines?: readonly RoutineSummary[];
  sequences?: readonly SequenceSummary[];
  /** Its items are being fetched. */
  loading: boolean;
  /** Why its items could not be loaded. */
  error: string | null;
}

export interface LazyRowOptions {
  search: string;
  strategy?: MatchStrategy;
  pinned?: ReadonlySet<string>;
  recent?: readonly string[];
  collapsed?: ReadonlySet<SidebarSection>;
  groups?: readonly SidebarGroup[];
}

/** The key of a schema node's own row (its rows are keyed `nodePrefix(schema)…`). */
export function nodeKey(schema: string): string {
  return `n:${encodeURIComponent(schema)}`;
}

function prefixed(row: TreeRow, prefix: string, schema: string): TreeRow {
  switch (row.type) {
    case "header":
      return { ...row, key: prefix + row.key, schema };
    case "column":
    case "columnStatus":
    case "nodeStatus":
      return { ...row, key: prefix + row.key, parentKey: prefix + row.parentKey };
    default:
      return { ...row, key: prefix + row.key };
  }
}

/** How many objects a node's rows hold (the section and group counts, matching the search). */
function itemCount(rows: readonly TreeRow[]): number {
  let n = 0;
  for (const r of rows) if (r.type === "header" || r.type === "groupHeader") n += r.count;
  return n;
}

/**
 * The lazy-loaded items tree (TablePlus "Lazy Load Items in Schema"): a "Recent" section built from
 * the loaded nodes' tables, then one node per schema / database. An expanded node lists its own
 * sections (Pinned, its groups, Functions, Tables, Views…, Sequences, as `buildTreeRows` /
 * `mergeObjectRows` build them, keyed under `nodePrefix`), or a loading / error row until its
 * items are in. A group shows only in the nodes that hold its items. While searching, loaded
 * nodes without a match leave the list; nodes not loaded stay (their items are not searched).
 */
export function buildLazyRows(nodes: readonly LazyNode[], o: LazyRowOptions): TreeRow[] {
  const strategy = o.strategy ?? "fuzzy";
  const collapsed = o.collapsed ?? EMPTY_COLLAPSED;
  const recent = o.recent ?? EMPTY_RECENT;
  const searching = isSearching(o.search, strategy);
  const rows: TreeRow[] = [];
  if (!searching && recent.length > 0) {
    const loaded = nodes.flatMap((n) => n.tables ?? []);
    for (const r of buildTreeRows(loaded, "", EMPTY_PINS, recent, collapsed)) {
      if (r.type === "header" && r.label !== "Recent") break;
      rows.push(r);
    }
  }
  for (const n of nodes) {
    let inner: TreeRow[] | null = null;
    if (n.tables !== undefined) {
      const tables = buildTreeRows(
        [...n.tables],
        o.search,
        o.pinned ?? EMPTY_PINS,
        EMPTY_RECENT,
        collapsed,
        o.groups ?? EMPTY_GROUPS,
        strategy,
      );
      inner = mergeObjectRows(
        tables,
        n.routines,
        n.sequences,
        o.search,
        collapsed,
        o.groups ?? EMPTY_GROUPS,
        strategy,
      ).filter((r) => r.type !== "groupHeader" || r.count > 0);
    }
    const count = inner === null ? null : itemCount(inner);
    if (searching && count === 0) continue;
    const key = nodeKey(n.schema);
    rows.push({
      type: "schemaNode",
      key,
      schema: n.schema,
      expanded: n.expanded,
      count,
      loading: n.loading && inner === null,
      system: n.system,
    });
    if (!n.expanded) continue;
    if (inner === null) {
      rows.push({
        type: "nodeStatus",
        key: `${key}#status`,
        parentKey: key,
        status: n.error !== null && !n.loading ? "error" : "loading",
        message: n.error,
      });
      continue;
    }
    const prefix = nodePrefix(n.schema);
    for (const r of inner) rows.push(prefixed(r, prefix, n.schema));
  }
  return rows;
}
