import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { TabBody } from "@/features/tabs/TabContent";
import { ipc, type ConnectionInfo, type MetricsQuery } from "@/lib/ipc";
import { resetMockMetrics } from "@/lib/ipc/mock/metrics";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore, type ToolTab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { MetricsHost } from "./MetricsHost";
import { defaultObject } from "./model";
import { AUTOMATIC_WRITE_REFUSED, WRITE_REFUSED } from "./runner";
import { createBoard, resetMetricsStoreForTests, useMetricsStore } from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

let info: ConnectionInfo;
const widthDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");

beforeAll(() => {
  // jsdom lays nothing out: give the canvas a width so its objects are placed.
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get: () => 952,
  });
});
afterAll(() => {
  if (widthDescriptor) Object.defineProperty(HTMLElement.prototype, "clientWidth", widthDescriptor);
});

beforeEach(async () => {
  resetMockMetrics();
  resetMetricsStoreForTests();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
  useUiStore.setState({ view: "launcher" });
  info = await ipc.connections.connect("p-local-pg");
  const store = useConnectionsStore.getState();
  store.add(info);
  store.setActive(info.connectionId);
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

function metricsTab(): ToolTab {
  const tab = useTabsStore.getState().tabs.find((t) => t.kind === "tool");
  if (!tab || tab.kind !== "tool") throw new Error("no metrics tab");
  return tab;
}

/** Opens the Metrics Board through its action (toolbar / View menu / palette) and shows it. */
async function openBoardTab() {
  render(<MetricsHost />);
  expect(actions.has("metrics.open")).toBe(true);
  await act(() => actions.run("metrics.open"));
  const view = render(<TabBody tab={metricsTab()} />);
  return view;
}

describe("Metrics Board tab", () => {
  it("opens from metrics.open as one tab per connection, and lists the boards", async () => {
    await openBoardTab();
    await act(() => actions.run("metrics.open"));
    expect(useTabsStore.getState().tabs).toHaveLength(1);
    expect(metricsTab()).toMatchObject({
      connectionId: info.connectionId,
      tool: "metrics",
      title: "Metrics Board",
    });
    expect(useUiStore.getState().view).toBe("workspace");
    const list = await screen.findByRole("listbox", { name: "Boards" });
    expect(await within(list).findByText("Sales overview")).toBeInTheDocument();
    // The demo board's scoreboards show a number once their queries ran.
    const users = await screen.findByRole("group", { name: "Users" });
    await waitFor(() => expect(within(users).getByText(/^\d[\d,.]*K?$/)).toBeInTheDocument());
  });

  it("shows a refused write inside the object", async () => {
    await createBoard(null, "Danger", [
      { ...defaultObject("table", "t1", "Wipe"), sql: "DELETE FROM users" },
    ]);
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Danger/ }));
    const wipe = await screen.findByRole("group", { name: "Wipe" });
    expect(await within(wipe).findByRole("alert")).toHaveTextContent(WRITE_REFUSED);
  });

  it("re-runs a wired object with the input's new value", async () => {
    await createBoard(null, "Search", [
      {
        ...defaultObject("input", "in", "Name"),
        variable: "q",
        defaultValue: "a",
        targets: ["t"],
      },
      {
        ...defaultObject("table", "t", "People"),
        sql: "SELECT id, name FROM users WHERE name LIKE {{q}}",
        refresh: { mode: "event" },
        layout: { x: 0, y: 2, w: 6, h: 6 },
      },
    ]);
    const run = vi.spyOn(ipc.metrics, "run");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Search/ }));
    await waitFor(() =>
      expect(run).toHaveBeenCalledWith(
        info.connectionId,
        expect.any(String),
        expect.objectContaining({ values: [{ t: "text", v: "a" }] }),
        true,
      ),
    );
    const input = within(await screen.findByRole("group", { name: "Name" })).getByRole("textbox");
    fireEvent.change(input, { target: { value: "bob" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(run).toHaveBeenLastCalledWith(
        info.connectionId,
        expect.any(String),
        expect.objectContaining({ values: [{ t: "text", v: "bob" }] }),
        true,
      ),
    );
  });

  it("adds an object in edit mode and opens its settings", async () => {
    await createBoard(null, "Empty");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Empty/ }));
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    await userEvent.click(screen.getByRole("button", { name: "Add object" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Scoreboard" }));
    const settings = await screen.findByRole("complementary", { name: "Scoreboard settings" });
    expect(within(settings).getByLabelText("Title")).toHaveValue("Scoreboard 1");
    expect(screen.getByRole("group", { name: "Scoreboard 1" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("complementary", { name: "Scoreboard settings" })).toBeNull();
  });

  it("moves objects on the grid and wires an input with its green dot", async () => {
    const board = await createBoard(null, "Wiring", [
      { ...defaultObject("input", "in", "Name"), variable: "q" },
      {
        ...defaultObject("table", "t", "People"),
        sql: "SELECT 1",
        layout: { x: 0, y: 2, w: 6, h: 6 },
      },
    ]);
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Wiring/ }));
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    const objects = () => useMetricsStore.getState().boards[board.id]!.objects;

    // Drag the table three columns right (a column is 72 px + an 8 px gutter here).
    const table = screen.getByRole("group", { name: "People" });
    fireEvent.pointerDown(table, { button: 0, clientX: 10, clientY: 10 });
    fireEvent.pointerMove(window, { clientX: 10 + 3 * 80 + 20, clientY: 14 });
    fireEvent.pointerUp(window, { clientX: 10 + 3 * 80 + 20, clientY: 14 });
    expect(objects().find((o) => o.id === "t")!.layout).toEqual({ x: 3, y: 2, w: 6, h: 6 });

    // Drop the input's green dot on the table: wired, and the table refreshes on event.
    const elementFromPoint = vi.fn(() => table);
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: elementFromPoint,
    });
    fireEvent.pointerDown(screen.getByRole("button", { name: "Wire Name to an object" }), {
      button: 0,
      clientX: 200,
      clientY: 40,
    });
    fireEvent.pointerMove(window, { clientX: 300, clientY: 200 });
    fireEvent.pointerUp(window, { clientX: 300, clientY: 200 });
    expect(objects().find((o) => o.id === "in")!.targets).toEqual(["t"]);
    expect(objects().find((o) => o.id === "t")!.refresh).toEqual({ mode: "event" });
    expect(toast.info).toHaveBeenCalledWith("“People” now refreshes on event", expect.anything());
  });

  it("creates a board and names it in place", async () => {
    await openBoardTab();
    await screen.findByRole("button", { name: /^Sales overview/ });
    await userEvent.click(screen.getAllByRole("button", { name: "New Board…" })[0]!);
    const name = await screen.findByRole("textbox", { name: "Board name" });
    expect(name).toHaveValue("Untitled Board");
    await userEvent.clear(name);
    await userEvent.type(name, "Revenue{Enter}");
    expect(await screen.findByRole("button", { name: /^Revenue/ })).toBeInTheDocument();
    // A new board opens in edit mode.
    expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
    await waitFor(() =>
      expect(Object.values(useMetricsStore.getState().boards).map((b) => b.name)).toContain(
        "Revenue",
      ),
    );
  });

  it("imports an encrypted board file after asking for its password", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/encrypted.tablerboard");
    await openBoardTab();
    await screen.findByRole("button", { name: /^Sales overview/ });
    await userEvent.click(screen.getByRole("button", { name: "More board actions" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Import Board…" }));
    const dialog = await screen.findByRole("dialog", { name: "Import Board" });
    const password = within(dialog).getByLabelText("Password");
    await userEvent.type(password, "nope{Enter}");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "The file password is wrong",
    );
    await userEvent.clear(password);
    await userEvent.type(password, "secret{Enter}");
    expect(await screen.findByRole("button", { name: /^Imported board/ })).toBeInTheDocument();
    expect(toast.success).toHaveBeenCalledWith("Imported “Imported board”");
  });

  /** The SQL a recorded `ipc.metrics.run` call sent. */
  const sqlOf = (call: unknown[]) => (call[2] as { parts: string[] }).parts.join("");

  it("does not run a write by itself when its board opens or when its timer fires", async () => {
    // What a board file from someone else looks like once imported: a write that is switched on.
    await createBoard(null, "Imported", [
      {
        ...defaultObject("table", "wipe", "Wipe"),
        sql: "DELETE FROM users",
        allowWrites: true,
        refresh: { mode: "interval", seconds: 1 },
      },
    ]);
    const run = vi.spyOn(ipc.metrics, "run");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Imported/ }));
    const wipe = await screen.findByRole("group", { name: "Wipe" });
    expect(await within(wipe).findByRole("alert")).toHaveTextContent(AUTOMATIC_WRITE_REFUSED);
    // The timer comes round twice and still nothing is sent for it.
    await new Promise((resolve) => setTimeout(resolve, 2_300));
    expect(run.mock.calls.filter((c) => sqlOf(c).includes("DELETE"))).toEqual([]);
  }, 20_000);

  it("runs that write when the user asks for it with Refresh", async () => {
    await createBoard(null, "Imported", [
      {
        ...defaultObject("table", "wipe", "Wipe"),
        sql: "DELETE FROM users",
        allowWrites: true,
      },
    ]);
    const run = vi.spyOn(ipc.metrics, "run");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Imported/ }));
    const wipe = await screen.findByRole("group", { name: "Wipe" });
    await within(wipe).findByRole("alert");
    const writes = () => run.mock.calls.filter((c) => sqlOf(c).includes("DELETE"));
    expect(writes()).toHaveLength(0);
    await userEvent.click(screen.getByRole("button", { name: "Refresh all" }));
    await waitFor(() => expect(writes()).toHaveLength(1));
    expect(writes()[0]![2]).toMatchObject({ allowWrites: true });
    await waitFor(() => expect(within(wipe).queryByRole("alert")).toBeNull());
  });

  it("does not run a write when a dropdown that settles on open refreshes it", async () => {
    await createBoard(null, "Chained", [
      {
        ...defaultObject("dropdown", "dd", "Role"),
        variable: "role",
        options: [{ value: "admin", label: "Admin" }],
        targets: ["wipe"],
      },
      {
        ...defaultObject("table", "wipe", "Wipe"),
        sql: "DELETE FROM users WHERE role = {{role}}",
        allowWrites: true,
        refresh: { mode: "event" },
        layout: { x: 0, y: 2, w: 6, h: 6 },
      },
    ]);
    const run = vi.spyOn(ipc.metrics, "run");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Chained/ }));
    const wipe = await screen.findByRole("group", { name: "Wipe" });
    expect(await within(wipe).findByRole("alert")).toHaveTextContent(AUTOMATIC_WRITE_REFUSED);
    expect(run.mock.calls.filter((c) => sqlOf(c).includes("DELETE"))).toEqual([]);
  });

  it("refreshes the objects wired to a button when it is clicked", async () => {
    await createBoard(null, "Buttons", [
      { ...defaultObject("button", "go", "Reload"), targets: ["t"] },
      {
        ...defaultObject("table", "t", "People"),
        sql: "SELECT id, name FROM users WHERE id > 0",
        refresh: { mode: "event" },
        layout: { x: 0, y: 2, w: 6, h: 6 },
      },
    ]);
    const run = vi.spyOn(ipc.metrics, "run");
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Buttons/ }));
    const people = () => run.mock.calls.filter((c) => sqlOf(c).includes("WHERE id > 0"));
    await waitFor(() => expect(people()).toHaveLength(1));
    await userEvent.click(await screen.findByRole("button", { name: "Reload" }));
    await waitFor(() => expect(people()).toHaveLength(2));
    // The button has no query of its own: nothing else ran for it.
    expect(run.mock.calls.filter((c) => sqlOf(c) === "")).toEqual([]);
  });

  it("gives every object a query timeout setting that the run sends", async () => {
    await createBoard(null, "Slow", [
      {
        ...defaultObject("table", "t", "People"),
        sql: "SELECT id, name FROM users WHERE id > 0",
      },
    ]);
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Slow/ }));
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.focus(await screen.findByRole("group", { name: "People" }));
    const settings = await screen.findByRole("complementary", { name: "Data table settings" });
    const timeout = within(settings).getByLabelText("Query timeout");
    // Boards from before the setting existed run with 60 s.
    expect(timeout).toHaveValue(60);
    const stored = () =>
      Object.values(useMetricsStore.getState().boards)
        .find((b) => b.name === "Slow")!
        .objects.find((o) => o.id === "t")!.timeoutSecs;
    expect(stored()).toBe(60);

    fireEvent.change(timeout, { target: { value: "90" } });
    expect(stored()).toBe(90);
    // Out of range: the nearest valid value when the box is left.
    fireEvent.change(timeout, { target: { value: "3" } });
    expect(stored()).toBe(90);
    fireEvent.blur(timeout);
    expect(timeout).toHaveValue(5);
    expect(stored()).toBe(5);
    fireEvent.change(timeout, { target: { value: "9000" } });
    fireEvent.blur(timeout);
    expect(timeout).toHaveValue(600);
    expect(stored()).toBe(600);
    fireEvent.change(timeout, { target: { value: "" } });
    fireEvent.blur(timeout);
    expect(timeout).toHaveValue(600);

    fireEvent.change(timeout, { target: { value: "45" } });
    const run = vi.spyOn(ipc.metrics, "run");
    await userEvent.click(within(settings).getByRole("button", { name: /^Run/ }));
    await waitFor(() => expect(run).toHaveBeenCalled());
    expect(run.mock.calls.at(-1)![2]).toMatchObject({ timeoutSecs: 45 });
  });

  it("shows a timed out query inside its object", async () => {
    await createBoard(null, "Timeouts", [
      {
        ...defaultObject("table", "default", "Default limit"),
        sql: "SELECT pg_sleep(100)",
        allowWrites: true,
      },
      {
        ...defaultObject("table", "short", "Short limit"),
        sql: "SELECT pg_sleep(50)",
        allowWrites: true,
        timeoutSecs: 5,
        layout: { x: 6, y: 0, w: 6, h: 6 },
      },
    ]);
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Timeouts/ }));
    const long = await screen.findByRole("group", { name: "Default limit" });
    expect(await within(long).findByRole("alert")).toHaveTextContent("Query timed out after 60 s");
    const short = await screen.findByRole("group", { name: "Short limit" });
    expect(await within(short).findByRole("alert")).toHaveTextContent("Query timed out after 5 s");
  });

  it("cancels the queries still running when the tab is closed", async () => {
    await createBoard(null, "Long runs", [
      {
        ...defaultObject("table", "t", "People"),
        sql: "SELECT id, name FROM users WHERE id > 0",
        timeoutSecs: 300,
      },
    ]);
    // A query that never answers on its own: only the backend's timeout or a cancel ends it.
    const run = vi.spyOn(ipc.metrics, "run").mockImplementation(() => new Promise(() => {}));
    const cancel = vi.spyOn(ipc.query, "cancel").mockResolvedValue(undefined);
    const view = await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Long runs/ }));
    await waitFor(() => expect(run).toHaveBeenCalledOnce());
    expect(run.mock.calls[0]![2]).toMatchObject({ timeoutSecs: 300 });
    expect(cancel).not.toHaveBeenCalled();
    view.unmount();
    expect(cancel).toHaveBeenCalledOnce();
    expect(cancel).toHaveBeenCalledWith(info.connectionId, run.mock.calls[0]![1]);
  });

  it("refuses a read that calls a function with side effects unless data changes are allowed", async () => {
    await createBoard(null, "Functions", [
      { ...defaultObject("table", "t", "Counter"), sql: "SELECT nextval('orders_id_seq')" },
    ]);
    await openBoardTab();
    await userEvent.click(await screen.findByRole("button", { name: /^Functions/ }));
    const counter = await screen.findByRole("group", { name: "Counter" });
    expect(await within(counter).findByRole("alert")).toHaveTextContent(
      "calls a function that changes data",
    );
  });

  it("refuses every advisory-lock function unless data changes are allowed", async () => {
    // The whole family, as the backend's deny-list has it: each takes or releases a lock on the
    // worker connection (the shared and `try` forms too).
    const family = [
      "pg_advisory_lock",
      "pg_advisory_lock_shared",
      "pg_advisory_xact_lock",
      "pg_advisory_xact_lock_shared",
      "pg_try_advisory_lock",
      "pg_try_advisory_lock_shared",
      "pg_try_advisory_xact_lock",
      "pg_try_advisory_xact_lock_shared",
      "pg_advisory_unlock",
      "pg_advisory_unlock_shared",
      "pg_advisory_unlock_all",
    ];
    const query = (sql: string, allowWrites: boolean): MetricsQuery => ({
      parts: [sql],
      values: [],
      allowWrites,
      maxRows: 10,
    });
    for (const name of family) {
      const sql = name.endsWith("_all") ? `SELECT ${name}()` : `SELECT ${name}(1)`;
      await expect(
        ipc.metrics.run(info.connectionId, `q-${name}`, query(sql, false)),
        name,
      ).rejects.toMatchObject({
        message: expect.stringContaining("calls a function that changes data"),
      });
      // With "Allow data changes" on it runs.
      await expect(
        ipc.metrics.run(info.connectionId, `q-${name}-ok`, query(sql, true)),
        name,
      ).resolves.toBeTruthy();
    }
  });

  it("refuses a side-effect call written so that a keyword scan misses it", async () => {
    // What the server still runs as the function: MySQL executable comments (the end of the
    // comment, the version number glued to the name) and PostgreSQL Unicode-escaped names.
    const query = (sql: string, allowWrites: boolean): MetricsQuery => ({
      parts: [sql],
      values: [],
      allowWrites,
      maxRows: 10,
    });
    const refused = { message: expect.stringContaining("changes data") };
    const mysql = await ipc.connections.connect("p-shop-mysql");
    const cases: [ConnectionInfo, string[]][] = [
      [
        mysql,
        [
          "SELECT SLEEP/*!*/(1)",
          "SELECT /*!50000 GET_LOCK*/('a', 10)",
          "SELECT /*!SLEEP*/(1)",
          "SELECT /*!50000SLEEP*/(1)",
          "SELECT /*M!100000 SETVAL*/(s, 10)",
          "SELECT NEXT /*!*/ VALUE FOR s",
        ],
      ],
      [
        info,
        [
          String.raw`SELECT U&"set\0076al"('s', 1)`,
          String.raw`SELECT U&"pg_\+000074erminate_backend"(1)`,
          String.raw`SELECT U&"set!0076al" UESCAPE '!'('s', 1)`,
          String.raw`SELECT U&"set!0076al" UESCAPE $$!$$('s', 1)`,
          String.raw`SELECT U&"set\zval"('s', 1)`,
          "SELECT \"setval\"('s', 1)",
        ],
      ],
    ];
    for (const [connection, list] of cases) {
      for (const sql of list) {
        await expect(
          ipc.metrics.run(connection.connectionId, "q-hidden", query(sql, false)),
          sql,
        ).rejects.toMatchObject(refused);
        await expect(
          ipc.metrics.run(connection.connectionId, "q-hidden-ok", query(sql, true)),
          sql,
        ).resolves.toBeTruthy();
      }
    }
    // Names that only look like it are not calls.
    for (const sql of [
      String.raw`SELECT U&"abs"(1), U&"l\0065ngth"('abc')`,
      String.raw`SELECT U&"d\0061ta" FROM users`,
      "SELECT 1 /* sleeping(1) */",
    ])
      await expect(
        ipc.metrics.run(info.connectionId, "q-plain", query(sql, false)),
        sql,
      ).resolves.toBeTruthy();
  });

  it("cancels a write that outlives its query timeout too", async () => {
    const write = (timeoutSecs?: number): MetricsQuery => ({
      parts: ["UPDATE users SET name = 'x' WHERE pg_sleep(100) IS NOT NULL"],
      values: [],
      allowWrites: true,
      maxRows: 10,
      ...(timeoutSecs === undefined ? {} : { timeoutSecs }),
    });
    await expect(ipc.metrics.run(info.connectionId, "w-default", write())).rejects.toMatchObject({
      message: "Query timed out after 60 s",
    });
    await expect(ipc.metrics.run(info.connectionId, "w-short", write(5))).rejects.toMatchObject({
      message: "Query timed out after 5 s",
    });
    // A write that is quick enough is not disturbed.
    const quick: MetricsQuery = { ...write(), parts: ["UPDATE users SET name = 'x'"] };
    await expect(ipc.metrics.run(info.connectionId, "w-quick", quick)).resolves.toMatchObject({
      affected: 1,
    });
  });

  it("imports boards with 'Allow data changes' switched off", async () => {
    const out = await ipc.metrics.importBoards("/Users/me/shared.tablerboard", null, "p-local-pg");
    const objects = out.boards[0]!.objects as Array<{ allowWrites: unknown }>;
    expect(objects.length).toBeGreaterThan(0);
    expect(objects.every((o) => o.allowWrites === false)).toBe(true);
  });
});
