import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type Value } from "@/lib/ipc";
import type { ObjectMenuContext, SidebarItemRef } from "../types";
import lane from ".";
import { mongoConsole } from "./console";
import { mongoObjectMenu, splitIndexText } from "./menu";
import { MongoInspector } from "./MongoInspector";

// The lane's views: the console's language, the document inspector and a collection's menu.

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the lane module", () => {
  it("gives the MongoDB form, console, inspector and menu, and no SQL dialect", () => {
    expect(Object.keys(lane.kinds ?? {})).toEqual(["mongodb"]);
    const ui = lane.drivers?.mongodb;
    expect(ui?.console).toBe(mongoConsole);
    expect(ui?.Inspector).toBe(MongoInspector);
    expect(ui?.TableView).toBeUndefined();
    expect(ui?.StructureView).toBeUndefined();
    expect(lane.dialects).toBeUndefined();
  });
});

describe("the console", () => {
  it("splits, classifies and highlights the mongosh subset", () => {
    const text = "db.orders.find({status: 'new'})\n  .limit(5)\ndb.orders.deleteMany({})";
    expect(mongoConsole.split(text).map((r) => text.slice(r.start, r.end))).toEqual([
      "db.orders.find({status: 'new'})\n  .limit(5)",
      "db.orders.deleteMany({})",
    ]);
    expect(mongoConsole.classify("db.orders.find({status: 'new'})")).toBe("read");
    expect(mongoConsole.classify("db.orders.aggregate([{$out: 'x'}])")).toBe("write");
    expect(mongoConsole.classify("frobnicate")).toBe("write");
    expect(mongoConsole.fileExtension).toBe("js");
    expect(mongoConsole.language()).toBeTruthy();
  });
});

const oid = (hex: string): Value => ({ t: "other", v: { ty: "objectId", text: hex } });
const ID = "65f1a2b3c4d5e6f708192a3b";

describe("the inspector", () => {
  const row: Record<string, Value> = {
    _id: oid(ID),
    name: { t: "text", v: "Ada" },
    n: { t: "int", v: 1 },
    gone: { t: "other", v: { ty: "missing", text: "" } },
  };
  const table = { schema: "shop", name: "people" };

  it("shows the document as Extended JSON and stages only what changed", () => {
    const onStage = vi.fn();
    render(
      <MongoInspector connectionId="c" table={table} columns={[]} row={row} onStage={onStage} />,
    );
    const area = screen.getByLabelText("Document") as HTMLTextAreaElement;
    expect(area.value).toContain(`"$oid": "${ID}"`);
    expect(area.value).not.toContain("gone");
    fireEvent.change(area, {
      target: { value: area.value.replace('"Ada"', '"Grace"').replace(/,\s*"n": 1/, "") },
    });
    fireEvent.click(screen.getByRole("button", { name: /^Stage/ }));
    expect(onStage).toHaveBeenCalledWith({
      name: { t: "text", v: "Grace" },
      n: { t: "other", v: { ty: "missing", text: "" } },
    });
  });

  it("refuses a change of _id, is read-only without onStage, and says when nothing is selected", () => {
    const onStage = vi.fn();
    const { unmount } = render(
      <MongoInspector connectionId="c" table={table} columns={[]} row={row} onStage={onStage} />,
    );
    const area = screen.getByLabelText("Document") as HTMLTextAreaElement;
    fireEvent.change(area, {
      target: { value: area.value.replace(ID, "65f1a2b3c4d5e6f708192a3c") },
    });
    expect(screen.getByRole("button", { name: /^Stage/ })).toBeDisabled();
    unmount();
    render(<MongoInspector connectionId="c" table={table} columns={[]} row={row} />);
    expect(screen.queryByRole("button", { name: /^Stage/ })).toBeNull();
    render(<MongoInspector connectionId="c" table={table} columns={[]} row={null} />);
    expect(screen.getByTestId("mongo-no-document")).toBeInTheDocument();
  });
});

describe("a collection's menu", () => {
  const info = (readOnly: boolean): ConnectionInfo =>
    ({ connectionId: "c1", currentDatabase: "shop", readOnly }) as ConnectionInfo;
  const item = (kind: SidebarItemRef["kind"], name = "orders"): SidebarItemRef => ({
    table: { schema: "shop", name },
    kind,
    database: "shop",
  });
  const ctx = (readOnly = false) => {
    const call = vi.fn().mockResolvedValue({ command: "x", reply: { ok: 1 } });
    const refresh = vi.fn();
    const c = { connectionId: "c1", info: info(readOnly), caps: {}, call, refresh };
    return { c: c as unknown as ObjectMenuContext, call, refresh };
  };

  it("offers the writes on a collection, fewer on a view, only stats on a system one", () => {
    const { c } = ctx();
    expect(mongoObjectMenu(item("table"), c).map((i) => i.label)).toEqual([
      "Collection Stats",
      "Create Index…",
      "Drop Index…",
      "Edit Validator…",
      "Rename…",
      "Truncate…",
      "Drop Collection…",
    ]);
    expect(mongoObjectMenu(item("view"), c).map((i) => i.label)).toEqual(["Drop View…"]);
    expect(mongoObjectMenu(item("systemView", "system.views"), c).map((i) => i.label)).toEqual([
      "Collection Stats",
    ]);
  });

  it("turns the writes off on a read-only connection", () => {
    const items = mongoObjectMenu(item("table"), ctx(true).c);
    expect(items.filter((i) => i.disabled).map((i) => i.id)).toEqual([
      "create-index",
      "drop-index",
      "validator",
      "rename",
      "truncate",
      "drop",
    ]);
  });

  it("reads stats without Safe Mode, the call's own way", async () => {
    const read = vi.spyOn(ipc, "sessionCall").mockResolvedValue({
      storageStats: { count: 3, size: 10, storageSize: 4096, totalIndexSize: 0 },
    });
    const { c, call } = ctx(true);
    await mongoObjectMenu(item("table"), c)[0]!.run();
    expect(read).toHaveBeenCalledWith("c1", "collectionStats", {
      database: "shop",
      collection: "orders",
    });
    expect(call).not.toHaveBeenCalled();
  });

  it("confirms a drop with the backend's command, then sends it through ctx.call", async () => {
    vi.spyOn(ipc, "sessionCall").mockResolvedValue({
      database: "shop",
      command: 'db.getSiblingDB("shop").runCommand({"drop": "orders"})',
    });
    const { c, call, refresh } = ctx();
    const drop = mongoObjectMenu(item("table"), c).find((i) => i.id === "drop")!;
    const running = drop.run();
    expect(await screen.findByTestId("mongo-command")).toHaveTextContent('{"drop": "orders"}');
    fireEvent.click(screen.getByRole("button", { name: "Drop Collection" }));
    await running;
    expect(call).toHaveBeenCalledWith("dropCollection", { database: "shop", collection: "orders" });
    expect(refresh).toHaveBeenCalled();
  });

  it("sends nothing when a question is declined", async () => {
    const { c, call } = ctx();
    const rename = mongoObjectMenu(item("table"), c).find((i) => i.id === "rename")!;
    const running = rename.run();
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await running;
    expect(call).not.toHaveBeenCalled();
  });

  it("creates an index from its keys and options", async () => {
    const { c, call } = ctx();
    const create = mongoObjectMenu(item("table"), c).find((i) => i.id === "create-index")!;
    const running = create.run();
    fireEvent.change(await screen.findByLabelText("Index"), {
      target: { value: "{email: 1}, {unique: true}" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await running;
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("createIndex", {
        database: "shop",
        collection: "orders",
        keys: "{email: 1}",
        options: "{unique: true}",
      }),
    );
  });

  it("splits an index's text into keys and options", () => {
    expect(splitIndexText("{a: 1, b: -1}")).toEqual({ keys: "{a: 1, b: -1}", options: "" });
    expect(splitIndexText(" {a: 'text'} , {name: 'x'} ")).toEqual({
      keys: "{a: 'text'}",
      options: "{name: 'x'}",
    });
    expect(splitIndexText("{a:")).toEqual({ keys: "{a:", options: "" });
  });
});
