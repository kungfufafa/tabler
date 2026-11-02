import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type Value } from "@/lib/ipc";
import type { RedisKeyPage } from "@/lib/ipc/bindings/RedisKeyPage";
import type { ObjectMenuContext, SidebarItemRef } from "../types";
import lane from ".";
import { redisConsole } from "./console";
import { flushCommand, redisObjectMenu, serverInfoText, slowLogText } from "./menu";
import { newValueOf } from "./newKey";
import { RedisInspector } from "./RedisInspector";

const guard = vi.hoisted(() => vi.fn(async () => true));
vi.mock("@/features/safety/guard", () => ({ guardExecution: guard }));

// The lane's views: the module's slots, the per-type key inspector (staged string and TTL edits,
// container pages and element edits behind Safe Mode) and a database's menu.

afterEach(() => {
  vi.restoreAllMocks();
  guard.mockClear();
  guard.mockImplementation(async () => true);
  document.body.innerHTML = "";
});

const text = (v: string): Value => ({ t: "text", v });
const table = { schema: null, name: "db3" };

describe("the lane module", () => {
  it("gives the Redis form, console, inspector and menu, and no SQL dialect or table view", () => {
    expect(Object.keys(lane.kinds ?? {})).toEqual(["redis"]);
    const ui = lane.drivers?.redis;
    expect(ui?.console).toBe(redisConsole);
    expect(ui?.Inspector).toBe(RedisInspector);
    expect(ui?.objectMenu).toBe(redisObjectMenu);
    // The app's grid is the key browser; the Structure tab shows `definition()`.
    expect(ui?.TableView).toBeUndefined();
    expect(ui?.StructureView).toBeUndefined();
    expect(lane.dialects).toBeUndefined();
  });
});

describe("the inspector", () => {
  const stringRow = (value: Value = text("hello")) => ({
    key: text("greeting"),
    type: text("string"),
    ttl: { t: "int", v: 60 } as Value,
    value,
  });

  it("stages a string's value and its TTL as grid edits", () => {
    const onStage = vi.fn();
    render(
      <RedisInspector
        connectionId="c"
        table={table}
        columns={[]}
        row={stringRow()}
        onStage={onStage}
      />,
    );
    expect(screen.getByTestId("redis-key")).toHaveTextContent("greeting");
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "hi there" } });
    fireEvent.click(screen.getByRole("button", { name: "Stage value" }));
    expect(onStage).toHaveBeenCalledWith({ value: text("hi there") });
    fireEvent.change(screen.getByLabelText("TTL"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Stage TTL" }));
    expect(onStage).toHaveBeenLastCalledWith({ ttl: { t: "null" } });
  });

  it("refuses to edit a preview, a JSON document that is not JSON, and everything without onStage", () => {
    const onStage = vi.fn();
    const cut: Value = { t: "bytes", v: { base64: "eHh4", len: 40_000, truncated: true } };
    const { unmount } = render(
      <RedisInspector
        connectionId="c"
        table={table}
        columns={[]}
        row={stringRow(cut)}
        onStage={onStage}
      />,
    );
    expect(screen.getByTestId("redis-preview-only")).toHaveTextContent("only a preview is loaded");
    unmount();
    const doc = {
      key: text("doc"),
      type: text("ReJSON-RL"),
      ttl: { t: "null" } as Value,
      value: { t: "json", v: "{}" } as Value,
    };
    const second = render(
      <RedisInspector connectionId="c" table={table} columns={[]} row={doc} onStage={onStage} />,
    );
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "{oops" } });
    expect(screen.getByRole("button", { name: "Stage value" })).toBeDisabled();
    second.unmount();
    render(<RedisInspector connectionId="c" table={table} columns={[]} row={stringRow()} />);
    expect(screen.queryByRole("button", { name: /^Stage/ })).toBeNull();
    expect(screen.getByLabelText("TTL")).toBeDisabled();
    render(<RedisInspector connectionId="c" table={table} columns={[]} row={null} />);
    expect(screen.getByTestId("redis-no-key")).toBeInTheDocument();
    render(
      <RedisInspector
        connectionId="c"
        table={table}
        columns={[]}
        row={{ key: text("ts"), type: text("TSDB-TYPE"), ttl: { t: "null" }, value: { t: "null" } }}
      />,
    );
    expect(screen.getByTestId("redis-module-type")).toHaveTextContent("TSDB-TYPE");
  });

  const hashPage = (more: boolean): RedisKeyPage => ({
    key: text("user:1"),
    type: "hash",
    ttl: { t: "null" },
    size: { t: "int", v: 3 },
    value: null,
    elements: more
      ? [{ name: text("name"), value: text("Ada") }]
      : [{ name: text("plan"), value: text("free") }],
    nextCursor: more ? "17" : null,
  });

  it("pages a container with keyPage and edits an element behind Safe Mode", async () => {
    const calls: [string, unknown][] = [];
    vi.spyOn(ipc, "sessionCall").mockImplementation(async (_c, method, args) => {
      calls.push([method, args]);
      if (method === "keyPage")
        return hashPage((args as { cursor: string | null }).cursor === null) as never;
      if (method === "previewEdits")
        return ["WATCH user:1", "MULTI", "HDEL user:1 name", "EXEC"] as never;
      return { results: [{ ok: true, error: null }], preview: [] } as never;
    });
    const row = {
      key: text("user:1"),
      type: text("hash"),
      ttl: { t: "null" } as Value,
      value: text("{}"),
    };
    render(
      <RedisInspector connectionId="c" table={table} columns={[]} row={row} onStage={vi.fn()} />,
    );
    expect(await screen.findByText("Ada")).toBeInTheDocument();
    expect(calls[0]).toEqual([
      "keyPage",
      { db: 3, key: text("user:1"), type: "hash", cursor: null, count: 100 },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(await screen.findByText("free")).toBeInTheDocument();
    expect(calls[1]?.[1]).toMatchObject({ cursor: "17" });
    fireEvent.click(screen.getByRole("button", { name: "Delete name" }));
    await waitFor(() => expect(calls.some(([m]) => m === "editKey")).toBe(true));
    expect(guard).toHaveBeenCalledWith("c", {
      kind: "commit",
      statements: ["WATCH user:1", "MULTI", "HDEL user:1 name", "EXEC"],
    });
    expect(calls.find(([m]) => m === "editKey")?.[1]).toEqual({
      db: 3,
      key: text("user:1"),
      type: "hash",
      edits: [{ op: "hashDelete", field: text("name") }],
    });
  });

  it("sends nothing when Safe Mode is declined", async () => {
    guard.mockImplementation(async () => false);
    const methods: string[] = [];
    vi.spyOn(ipc, "sessionCall").mockImplementation(async (_c, method) => {
      methods.push(method);
      if (method === "keyPage") return hashPage(false) as never;
      return ["SADD s x"] as never;
    });
    const row = {
      key: text("s"),
      type: text("set"),
      ttl: { t: "null" } as Value,
      value: text("[]"),
    };
    render(
      <RedisInspector connectionId="c" table={table} columns={[]} row={row} onStage={vi.fn()} />,
    );
    await screen.findByText("plan");
    fireEvent.change(screen.getByLabelText("Member"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Add member" }));
    await waitFor(() => expect(guard).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 0));
    expect(methods).toEqual(["keyPage", "previewEdits"]);
  });
});

describe("a database's menu", () => {
  const item: SidebarItemRef = {
    table: { schema: null, name: "db2" },
    kind: "table",
    database: null,
  };
  const ctx = (readOnly = false) => {
    const call = vi.fn().mockResolvedValue({ results: [{ ok: true, error: null }], preview: [] });
    const read = vi.fn().mockResolvedValue(null);
    const refresh = vi.fn();
    const c = {
      connectionId: "c1",
      info: { connectionId: "c1", readOnly } as ConnectionInfo,
      caps: {},
      call,
      read,
      refresh,
    };
    return { c: c as unknown as ObjectMenuContext, call, read, refresh };
  };

  it("offers the reads, and the writes only on a writable connection", () => {
    expect(redisObjectMenu(item, ctx().c).map((i) => [i.label, i.disabled ?? false])).toEqual([
      ["Server Info", false],
      ["Slow Log", false],
      ["New Key…", false],
      ["Flush Database…", false],
    ]);
    expect(
      redisObjectMenu(item, ctx(true).c)
        .filter((i) => i.disabled)
        .map((i) => i.id),
    ).toEqual(["new-key", "flush"]);
  });

  it("reads the server info through ctx.read", async () => {
    const { c, read, call } = ctx(true);
    read.mockResolvedValue({
      product: "Valkey",
      version: "8.0.1",
      mode: "standalone",
      protocol: 3,
      databases: 16,
      readOnly: true,
      sections: [{ name: "Server", fields: [["os", "Linux"]] }],
    });
    const running = redisObjectMenu(item, c)[0]!.run();
    expect(await screen.findByTestId("redis-command")).toHaveTextContent(
      "Valkey 8.0.1 (standalone), RESP3",
    );
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await running;
    expect(read).toHaveBeenCalledWith("serverInfo", null);
    expect(call).not.toHaveBeenCalled();
  });

  it("confirms a flush with its command, then sends it through ctx.call", async () => {
    const { c, call, refresh } = ctx();
    const running = redisObjectMenu(item, c)
      .find((i) => i.id === "flush")!
      .run();
    expect(await screen.findByTestId("redis-command")).toHaveTextContent("FLUSHDB ASYNC");
    fireEvent.click(screen.getByRole("button", { name: "Flush Database" }));
    await running;
    expect(call).toHaveBeenCalledWith("flushDb", { db: 2 });
    expect(refresh).toHaveBeenCalled();
  });

  it("shows FLUSHDB without ASYNC when the server is older than Redis 4", async () => {
    const { c, call, read } = ctx();
    const server = (version: string) => ({
      product: "Redis",
      version,
      mode: "standalone",
      protocol: 2,
      databases: 16,
      readOnly: false,
      sections: [{ name: "Server", fields: [["redis_version", version]] }],
    });
    read.mockResolvedValue(server("3.2.12"));
    const running = redisObjectMenu(item, c)
      .find((i) => i.id === "flush")!
      .run();
    const command = await screen.findByTestId("redis-command");
    expect(command.textContent).not.toContain("ASYNC");
    expect(command).toHaveTextContent("FLUSHDB");
    fireEvent.click(screen.getByRole("button", { name: "Flush Database" }));
    await running;
    expect(read).toHaveBeenCalledWith("serverInfo", null);
    expect(call).toHaveBeenCalledWith("flushDb", { db: 2 });
    // Like Server::gates: from 4.0, and a hidden version is taken for a current server.
    expect(flushCommand(2, server("4.0.0") as never)).toBe("SELECT 2\nFLUSHDB ASYNC");
    expect(flushCommand(2, server("v3.0.7") as never)).toBe("SELECT 2\nFLUSHDB");
    expect(flushCommand(0, null)).toBe("SELECT 0\nFLUSHDB ASYNC");
  });

  it("creates a key from the New key dialog", async () => {
    const { c, call } = ctx();
    const running = redisObjectMenu(item, c)
      .find((i) => i.id === "new-key")!
      .run();
    fireEvent.change(await screen.findByLabelText("Key name"), { target: { value: "list:1" } });
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "x" } });
    fireEvent.change(screen.getByLabelText("TTL (seconds)"), { target: { value: "30" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await running;
    expect(call).toHaveBeenCalledWith("newKey", {
      db: 2,
      key: text("list:1"),
      ttl: { t: "int", v: 30 },
      value: { type: "string", value: text("x") },
    });
  });

  it("sends nothing when a question is declined", async () => {
    const { c, call } = ctx();
    const running = redisObjectMenu(item, c)
      .find((i) => i.id === "flush")!
      .run();
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await running;
    expect(call).not.toHaveBeenCalled();
  });

  it("writes the slow log and reads New key's value text by type", () => {
    expect(
      slowLogText([
        {
          id: { t: "int", v: 7 },
          startedAt: { t: "int", v: 0 },
          durationUs: { t: "int", v: 12_500 },
          command: "KEYS *",
          client: "10.0.0.1:5000",
          clientName: null,
        },
      ]),
    ).toBe("#7 1970-01-01T00:00:00Z 12.5 ms 10.0.0.1:5000\n  KEYS *");
    expect(slowLogText([])).toBe("The slow log is empty.");
    expect(
      serverInfoText({
        product: "Redis",
        version: "7",
        mode: "standalone",
        protocol: 2,
        databases: 1,
        readOnly: false,
        sections: [],
      }),
    ).toBe("Redis 7 (standalone), RESP2\n1 databases");
    expect(newValueOf("hash", "name Ada Lovelace\nage 36")).toEqual({
      ok: true,
      value: {
        type: "hash",
        fields: [
          { name: text("name"), value: text("Ada Lovelace") },
          { name: text("age"), value: text("36") },
        ],
      },
    });
    expect(newValueOf("zset", "1.5 a\n-inf b")).toMatchObject({ ok: true });
    expect(newValueOf("zset", "high a")).toEqual({
      ok: false,
      error: "“high” is not a score (a number, inf or -inf)",
    });
    expect(newValueOf("list", "\n")).toMatchObject({ ok: false });
    expect(newValueOf("json", "{")).toMatchObject({ ok: false });
    expect(newValueOf("stream", "f v")).toMatchObject({
      ok: true,
      value: { type: "stream", id: null },
    });
  });
});
