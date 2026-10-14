import type { ConnectionTag } from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import { isTableTab, type Tab } from "@/stores/tabs";
import { databaseLabel, serverLabel, TAG_STYLE } from "./connectionLabels";

/**
 * How eagerly a segment gives up its width when the pill is too narrow: the middle parts (SSH /
 * TLS, connection name, database, schema) truncate first, the server and the visible object last.
 */
export type SegmentRole =
  "state" | "server" | "transport" | "name" | "database" | "schema" | "object";

export interface PillSegment {
  role: SegmentRole;
  text: string;
}

export interface PillModel {
  tag: ConnectionTag | null;
  /** Lost / reconnecting: the pill takes the warning tint and leads with the state. */
  problem: "lost" | "reconnecting" | null;
  segments: PillSegment[];
}

/** The pill's tag text: TablePlus's short labels for the built-in tags, a custom tag's name. */
export function tagLabel(tag: ConnectionTag): string {
  return TAG_STYLE[tag]?.label ?? tag.toUpperCase();
}

/** `SSH` (tunnel), `TLS` (encrypted session), both as `SSH/TLS`; null for neither. */
export function transportLabel(ssh: boolean, tls: boolean): string | null {
  if (ssh && tls) return "SSH/TLS";
  if (ssh) return "SSH";
  return tls ? "TLS" : null;
}

/**
 * `TAG │ PostgreSQL 17.6 : SSH/TLS : <name> : <database> : <schema>.<table>` (or `SQL Query` for
 * a query tab; a tool tab such as the Metrics Board names no object), the toolbar's description of
 * where the visible tab points. `tls`: the session is encrypted (`ipc.security.sessionDetails`).
 */
export function pillModel(
  conn: OpenConnection,
  tab: Tab | null,
  tag: ConnectionTag | null,
  reconnecting: boolean,
  tls = false,
): PillModel {
  const { info } = conn;
  const problem =
    reconnecting || conn.status === "reconnecting"
      ? "reconnecting"
      : conn.status === "lost"
        ? "lost"
        : null;
  const segments: PillSegment[] = [];
  if (problem)
    segments.push({
      role: "state",
      text: problem === "reconnecting" ? "Reconnecting…" : "Connection lost",
    });
  segments.push({ role: "server", text: serverLabel(info) });
  const transport = transportLabel(info.tunnel !== null, tls);
  if (transport) segments.push({ role: "transport", text: transport });
  segments.push({ role: "name", text: info.name });
  const database = databaseLabel(info);
  if (database) segments.push({ role: "database", text: database });
  const own = tab?.connectionId === info.connectionId ? tab : null;
  if (isTableTab(own)) {
    const name = own.table.schema ? `${own.table.schema}.${own.table.name}` : own.table.name;
    segments.push({
      role: "object",
      text: own.kind === "structure" ? `${name} (Structure)` : name,
    });
  } else {
    // MySQL's "schema" is the database itself: show it once.
    if (conn.currentSchema && conn.currentSchema !== database)
      segments.push({ role: "schema", text: conn.currentSchema });
    // Only a query tab is "SQL Query": a feature's tool tab (Metrics Board, Item Overview, SQL
    // Viewer) is not.
    if (own?.kind === "query") segments.push({ role: "object", text: "SQL Query" });
  }
  return { tag, problem, segments };
}

export function pillText(model: PillModel): string {
  const body = model.segments.map((s) => s.text).join(" : ");
  return model.tag ? `${tagLabel(model.tag)} │ ${body}` : body;
}
