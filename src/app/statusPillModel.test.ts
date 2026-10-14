import { describe, expect, it } from "vitest";
import type { ConnectionInfo } from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import type { Tab } from "@/stores/tabs";
import { pillModel, pillText } from "./statusPillModel";

const info: ConnectionInfo = {
  connectionId: "a",
  profileId: "p-a",
  name: "Dev",
  color: "green",
  driver: "postgres",
  serverVersion: "PostgreSQL 17.6 (Homebrew) on aarch64-apple-darwin",
  currentDatabase: "db",
  currentUser: "postgres",
  tunnel: null,
  readOnly: false,
};

const conn: OpenConnection = {
  info,
  status: "connected",
  currentSchema: "public",
} as OpenConnection;

const query: Tab = { id: "q", kind: "query", connectionId: "a", title: "Query 1" };
const tool = (name: string, connectionId = "a"): Tab => ({
  id: `tool-${name}`,
  kind: "tool",
  connectionId,
  tool: name,
  title: name,
});

const BASE = "PostgreSQL 17.6 : Dev : db : public";

describe("the status pill's last segment", () => {
  it("says SQL Query for a query tab", () => {
    expect(pillText(pillModel(conn, query, null, false))).toBe(`${BASE} : SQL Query`);
  });

  it("names no object for a feature's tool tab", () => {
    for (const name of ["metrics", "item-overview", "sqlviewer"]) {
      const model = pillModel(conn, tool(name), null, false);
      expect(pillText(model), name).toBe(BASE);
      expect(model.segments.some((s) => s.role === "object")).toBe(false);
    }
  });

  it("still names a table and its structure", () => {
    const table = { schema: "public", name: "users" };
    const tab: Tab = { id: "t", kind: "table", connectionId: "a", table, title: "users" };
    expect(pillText(pillModel(conn, tab, null, false))).toBe(
      "PostgreSQL 17.6 : Dev : db : public.users",
    );
    expect(pillText(pillModel(conn, { ...tab, kind: "structure" }, null, false))).toBe(
      "PostgreSQL 17.6 : Dev : db : public.users (Structure)",
    );
  });

  it("leaves another connection's tabs, tool tabs included, out", () => {
    expect(pillText(pillModel(conn, tool("metrics", "b"), null, false))).toBe(BASE);
    expect(pillText(pillModel(conn, null, null, false))).toBe(BASE);
  });
});
