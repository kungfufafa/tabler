import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type Value } from "@/lib/ipc";
import type { ObjectMenuContext, SidebarItemRef } from "../types";
import lane from ".";
import { esConsole } from "./console";
import { ElasticInspector } from "./ElasticInspector";
import { esObjectMenu } from "./menu";

// The lane's views: the console's language, the hit inspector and an index's menu.

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the lane module", () => {
  it("gives the form, console, inspector and menu, and no SQL dialect", () => {
    expect(Object.keys(lane.kinds ?? {})).toEqual(["elasticsearch"]);
    const ui = lane.drivers?.elasticsearch;
    expect(ui?.console).toBe(esConsole);
    expect(ui?.Inspector).toBe(ElasticInspector);
    expect(ui?.TableView).toBeUndefined();
    expect(ui?.StructureView).toBeUndefined();
    expect(lane.dialects).toBeUndefined();
  });
});

describe("the console", () => {
  it("splits, classifies and highlights requests and SQL", () => {
    const text =
      'GET /logs/_search\n{"query": {"match_all": {}}}\n\nSELECT * FROM logs;\nDELETE /logs';
    expect(esConsole.split(text).map((r) => text.slice(r.start, r.end))).toEqual([
      'GET /logs/_search\n{"query": {"match_all": {}}}',
      "SELECT * FROM logs;",
      "DELETE /logs",
    ]);
    expect(esConsole.classify("GET /logs/_search")).toBe("read");
    expect(esConsole.classify("POST /logs/_doc\n{}")).toBe("write");
    expect(esConsole.classify("frobnicate")).toBe("write");
    expect(esConsole.fileExtension).toBe("es");
    expect(esConsole.language()).toBeTruthy();
  });
});

const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });

describe("the inspector", () => {
  const row: Record<string, Value> = {
    _id: text("1"),
    _seq_no: int(4),
    _primary_term: int(1),
    "user.name": text("Ada"),
    status: int(200),
    tags: { t: "other", v: { ty: "missing", text: "" } },
  };
  const table = { schema: null, name: "logs" };

  it("shows the hit as JSON and stages only what changed", () => {
    const onStage = vi.fn();
    render(
      <ElasticInspector connectionId="c" table={table} columns={[]} row={row} onStage={onStage} />,
    );
    const area = screen.getByLabelText("Document") as HTMLTextAreaElement;
    expect(area.value).toContain('"user.name": "Ada"');
    expect(area.value).not.toContain("tags");
    fireEvent.change(area, { target: { value: area.value.replace("200", "404") } });
    fireEvent.click(screen.getByRole("button", { name: /^Stage/ }));
    expect(onStage).toHaveBeenCalledWith({ status: int(404) });
  });

  it("refuses a change of the metadata or a removed field, is read-only without onStage", () => {
    const onStage = vi.fn();
    const { unmount } = render(
      <ElasticInspector connectionId="c" table={table} columns={[]} row={row} onStage={onStage} />,
    );
    const area = screen.getByLabelText("Document") as HTMLTextAreaElement;
    const original = area.value;
    fireEvent.change(area, { target: { value: original.replace('"_seq_no": 4', '"_seq_no": 5') } });
    expect(screen.getByRole("button", { name: /^Stage/ })).toBeDisabled();
    fireEvent.change(area, { target: { value: original.replace(/,\s*"status": 200/, "") } });
    expect(screen.getByRole("button", { name: /^Stage/ })).toBeDisabled();
    unmount();
    render(<ElasticInspector connectionId="c" table={table} columns={[]} row={row} />);
    expect(screen.queryByRole("button", { name: /^Stage/ })).toBeNull();
    render(<ElasticInspector connectionId="c" table={table} columns={[]} row={null} />);
    expect(screen.getByTestId("es-no-document")).toBeInTheDocument();
  });
});

describe("an index's menu", () => {
  const info = (readOnly: boolean): ConnectionInfo =>
    ({ connectionId: "c1", currentDatabase: "docker-cluster", readOnly }) as ConnectionInfo;
  const item = (kind: SidebarItemRef["kind"], name = "logs"): SidebarItemRef => ({
    table: { schema: null, name },
    kind,
    database: null,
  });
  const ctx = (readOnly = false) => {
    const call = vi.fn().mockResolvedValue({ request: "x", reply: "{}" });
    const refresh = vi.fn();
    const c = { connectionId: "c1", info: info(readOnly), caps: {}, call, refresh };
    return { c: c as unknown as ObjectMenuContext, call, refresh };
  };
  const labels = (i: SidebarItemRef, c = ctx().c) => esObjectMenu(i, c).map((x) => x.label);

  it("offers every operation on an index, fewer on a stream, alias or pattern", () => {
    expect(labels(item("table"))).toEqual([
      "Index Settings",
      "Refresh",
      "Flush",
      "Force Merge",
      "Clear Cache",
      "Open Index",
      "Close Index",
      "New Index…",
      "Truncate…",
      "Delete Index…",
    ]);
    expect(labels(item("partitioned", "events"))).toEqual([
      "Index Settings",
      "Refresh",
      "New Index…",
    ]);
    expect(labels(item("view", "current"))).toEqual(["Index Settings", "Refresh", "New Index…"]);
    expect(labels(item("table", "logs-*"))).toEqual(["Index Settings", "Refresh", "New Index…"]);
    expect(labels(item("systemView", ".kibana_1"))).toEqual(["Index Settings"]);
  });

  it("turns the writes off on a read-only connection", () => {
    const items = esObjectMenu(item("table"), ctx(true).c);
    expect(items.filter((i) => !i.disabled).map((i) => i.id)).toEqual(["settings"]);
  });

  it("reads the settings without Safe Mode, the call's own way", async () => {
    const read = vi.spyOn(ipc, "sessionCall").mockResolvedValue('{\n  "logs": {}\n}');
    const { c, call } = ctx(true);
    const running = esObjectMenu(item("table"), c)[0]!.run();
    expect(await screen.findByTestId("es-command")).toHaveTextContent('"logs"');
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await running;
    expect(read).toHaveBeenCalledWith("c1", "indexSettings", { index: "logs" });
    expect(call).not.toHaveBeenCalled();
  });

  it("runs an index action through ctx.call", async () => {
    const { c, call, refresh } = ctx();
    await esObjectMenu(item("table"), c)
      .find((i) => i.id === "forcemerge")!
      .run();
    expect(call).toHaveBeenCalledWith("indexAction", { index: "logs", action: "forcemerge" });
    expect(refresh).toHaveBeenCalled();
  });

  it("confirms a truncate with the request it sends, then sends it through ctx.call", async () => {
    const { c, call } = ctx();
    const running = esObjectMenu(item("table"), c)
      .find((i) => i.id === "truncate")!
      .run();
    expect(await screen.findByTestId("es-command")).toHaveTextContent(
      'POST /logs/_delete_by_query?conflicts=proceed&refresh=true {"query":{"match_all":{}}}',
    );
    fireEvent.click(screen.getByRole("button", { name: "Truncate" }));
    await running;
    expect(call).toHaveBeenCalledWith("truncate", { index: "logs" });
  });

  it("sends nothing when a question is declined", async () => {
    const { c, call } = ctx();
    const running = esObjectMenu(item("table"), c)
      .find((i) => i.id === "delete")!
      .run();
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await running;
    expect(call).not.toHaveBeenCalled();
  });

  it("creates an index from its name and body, and refuses a body that is not JSON", async () => {
    const { c, call } = ctx();
    const create = esObjectMenu(item("table"), c).find((i) => i.id === "create")!;
    let running = create.run();
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "logs-2" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Body"), {
      target: { value: '{"settings": {"number_of_shards": 1}}' },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await running;
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith("createIndex", {
        name: "logs-2",
        body: '{"settings": {"number_of_shards": 1}}',
      }),
    );
    call.mockClear();
    running = create.run();
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.change(await screen.findByLabelText("Body"), { target: { value: "{nope" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await expect(running).rejects.toThrow(/quoted key/);
    expect(call).not.toHaveBeenCalled();
  });
});
