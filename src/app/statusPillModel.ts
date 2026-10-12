import type { ConnectionTag } from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import type { Tab } from "@/stores/tabs";
import { databaseLabel, serverLabel, TAG_STYLE } from "./connectionLabels";

/**
 * How eagerly a segment gives up its width when the pill is too narrow: the middle parts (SSH,
 * connection name, database, schema) truncate first, the server and the visible object last.
 */
export type SegmentRole = "state" | "server" | "ssh" | "name" | "database" | "schema" | "object";

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

/**
 * `TAG │ PostgreSQL 17.6 : SSH : <name> : <database> : <schema>.<table>` (or `SQL Query` for a
 * query tab), the toolbar's description of where the visible tab points.
 */
export function pillModel(
  conn: OpenConnection,
  tab: Tab | null,
  tag: ConnectionTag | null,
  reconnecting: boolean,
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
  if (info.tunnel) segments.push({ role: "ssh", text: "SSH" });
  segments.push({ role: "name", text: info.name });
  const database = databaseLabel(info);
  if (database) segments.push({ role: "database", text: database });
  const own = tab?.connectionId === info.connectionId ? tab : null;
  if (own && own.kind !== "query") {
    const name = own.table.schema ? `${own.table.schema}.${own.table.name}` : own.table.name;
    segments.push({
      role: "object",
      text: own.kind === "structure" ? `${name} (Structure)` : name,
    });
  } else {
    // MySQL's "schema" is the database itself: show it once.
    if (conn.currentSchema && conn.currentSchema !== database)
      segments.push({ role: "schema", text: conn.currentSchema });
    if (own) segments.push({ role: "object", text: "SQL Query" });
  }
  return { tag, problem, segments };
}

export function pillText(model: PillModel): string {
  const body = model.segments.map((s) => s.text).join(" : ");
  return model.tag ? `${TAG_STYLE[model.tag].label} │ ${body}` : body;
}
