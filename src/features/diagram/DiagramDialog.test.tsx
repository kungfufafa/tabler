import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { DiagramHost } from "./DiagramHost";
import { closeDiagram, openDiagram, useDiagramStore } from "./store";
import { positionsKey } from "./viewport";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

let info: ConnectionInfo;

async function setup(tables: Array<{ schema: string; name: string }> | null = null) {
  info = await ipc.connections.connect("p-local-pg");
  const store = useConnectionsStore.getState();
  store.add(info);
  store.setActive(info.connectionId);
  store.setCurrentSchema(info.connectionId, "public");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <DiagramHost />
    </QueryClientProvider>,
  );
  act(() => openDiagram({ connectionId: info.connectionId, schema: "public", tables }));
  const dialog = await screen.findByRole("dialog");
  await waitFor(() => expect(dialog.querySelectorAll("[data-table]").length).toBeGreaterThan(0));
  return { dialog, qc };
}

const card = (dialog: HTMLElement, name: string) =>
  dialog.querySelector<SVGGElement>(`[data-table="${CSS.escape(name)}"]`);
const cards = (dialog: HTMLElement) =>
  [...dialog.querySelectorAll<SVGGElement>("[data-table]")].map((g) => g.dataset.table);
const nameOf = (dialog: HTMLElement, name: string) =>
  card(dialog, name)!.querySelector<SVGTextElement>("[data-table-name]")!;
const canvas = () => screen.getByRole("application", { name: "Diagram canvas" });

/** A press and release on a card without moving: a click. */
function clickCard(dialog: HTMLElement, name: string, init: PointerEventInit = {}) {
  fireEvent.pointerDown(card(dialog, name)!, { button: 0, clientX: 10, clientY: 10, ...init });
  fireEvent.pointerUp(window, { clientX: 10, clientY: 10 });
}

beforeEach(() => {
  localStorage.clear();
  useDiagramStore.setState({ request: null, seq: 0 });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
});

afterEach(async () => {
  act(() => closeDiagram());
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
  if (info) await ipc.connections.disconnect(info.connectionId).catch(() => undefined);
});

describe("Diagram", () => {
  it("draws every table of the schema with its relations", async () => {
    const { dialog } = await setup();
    expect(within(dialog).getByText("Diagram — public")).toBeInTheDocument();
    expect(cards(dialog)).toContain("users");
    expect(cards(dialog)).toContain("post_tags");
    expect(cards(dialog)).toHaveLength(12);
    expect(dialog.querySelectorAll("[data-relation]")).toHaveLength(3);
    expect(within(dialog).getByText("12 tables · 3 relations")).toBeInTheDocument();
    // Rows: PK first, the FK column of posts.
    const posts = card(dialog, "posts")!;
    expect(posts.querySelector('[data-column="user_id"]')).not.toBeNull();
  });

  it("highlights a selected table's relations and dims the rest", async () => {
    const { dialog } = await setup();
    clickCard(dialog, "posts");
    expect(card(dialog, "posts")).toHaveAttribute("data-selected", "true");
    // Its neighbours stay lit, unrelated tables are dimmed.
    expect(card(dialog, "users")).toHaveAttribute("opacity", "1");
    expect(card(dialog, "post_tags")).toHaveAttribute("opacity", "1");
    expect(card(dialog, "tags")).toHaveAttribute("opacity", "0.32");
    const lines = [...dialog.querySelectorAll<SVGGElement>("[data-relation]")];
    const opacity = (name: string) =>
      lines.find((l) => l.dataset.relation === name)?.getAttribute("opacity");
    expect(opacity("posts_user_id_fkey")).toBe("1");
    expect(opacity("post_tags_tag_id_fkey")).toBe("0.14");
    // ⌘-click adds to the selection; a click on the background clears it.
    clickCard(dialog, "tags", { metaKey: true });
    expect(card(dialog, "tags")).toHaveAttribute("data-selected", "true");
    expect(card(dialog, "posts")).toHaveAttribute("data-selected", "true");
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 5, clientY: 5 });
    fireEvent.pointerUp(window, { clientX: 5, clientY: 5 });
    expect(dialog.querySelectorAll("[data-selected]")).toHaveLength(0);
  });

  it("shows only the selected tables and their neighbours", async () => {
    const user = userEvent.setup();
    const { dialog } = await setup();
    clickCard(dialog, "posts");
    const toggle = within(dialog).getByRole("button", { name: /Selected \+ Neighbours/ });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(cards(dialog).sort()).toEqual(["post_tags", "posts", "users"]);
    expect(within(dialog).getByText("3 of 12 tables · 2 relations")).toBeInTheDocument();
    await user.click(toggle);
    expect(cards(dialog)).toHaveLength(12);
  });

  it("starts from the sidebar's tables with their neighbours", async () => {
    const { dialog } = await setup([{ schema: "public", name: "tags" }]);
    expect(cards(dialog).sort()).toEqual(["post_tags", "tags"]);
    expect(card(dialog, "tags")).toHaveAttribute("data-selected", "true");
    expect(within(dialog).getByRole("button", { name: /Selected \+ Neighbours/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    // A click on the background keeps the selection that decides what is shown.
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 5, clientY: 5 });
    fireEvent.pointerUp(window, { clientX: 5, clientY: 5 });
    expect(cards(dialog).sort()).toEqual(["post_tags", "tags"]);
    // Clicking a neighbour walks on: its own neighbours appear.
    clickCard(dialog, "post_tags");
    expect(cards(dialog).sort()).toEqual(["post_tags", "posts", "tags"]);
  });

  it("opens a table on a click on its name, its structure on a double-click", async () => {
    let { dialog } = await setup();
    fireEvent.click(nameOf(dialog, "users"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    let tab = useTabsStore.getState().tabs[0];
    expect(tab).toMatchObject({ kind: "table", table: { schema: "public", name: "users" } });

    act(() => openDiagram({ connectionId: info.connectionId, schema: "public", tables: null }));
    dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(card(dialog, "tags")).not.toBeNull());
    const name = nameOf(dialog, "tags");
    fireEvent.click(name);
    fireEvent.doubleClick(name);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    tab = useTabsStore.getState().tabs.find((t) => t.kind !== "query" && t.table.name === "tags");
    expect(tab).toMatchObject({ kind: "structure" });
    // The pending single click never fires.
    await new Promise((r) => setTimeout(r, 300));
    expect(useTabsStore.getState().tabs.filter((t) => t.kind !== "query")).toHaveLength(2);
  });

  it("moves a dragged card and remembers where", async () => {
    const { dialog } = await setup();
    const before = card(dialog, "users")!.getAttribute("transform")!;
    const [, x, y] = before.match(/translate\((-?[\d.]+) (-?[\d.]+)\)/)!.map(Number);
    fireEvent.pointerDown(card(dialog, "users")!, { button: 0, clientX: 100, clientY: 100 });
    fireEvent.pointerMove(window, { clientX: 160, clientY: 130 });
    fireEvent.pointerUp(window, { clientX: 160, clientY: 130 });
    expect(card(dialog, "users")).toHaveAttribute("transform", `translate(${x! + 60} ${y! + 30})`);
    // A drag is not a click: no selection.
    expect(card(dialog, "users")).not.toHaveAttribute("data-selected");
    const key = positionsKey(info.profileId, info.currentDatabase, "public");
    await waitFor(() =>
      expect(JSON.parse(localStorage.getItem(key) ?? "{}").users).toEqual({
        x: x! + 60,
        y: y! + 30,
      }),
    );
    // Auto Layout forgets it.
    fireEvent.click(within(dialog).getByRole("button", { name: /Auto Layout/ }));
    expect(card(dialog, "users")).toHaveAttribute("transform", before);
    expect(localStorage.getItem(key)).toBeNull();
  });

  it("pans with the background and the arrow keys, zooms with ⌘+ / ⌘− / ⌘0", async () => {
    const { dialog } = await setup();
    const viewport = () => dialog.querySelector("svg > g")!.getAttribute("transform");
    expect(viewport()).toBe("translate(0 0) scale(1)");
    fireEvent.pointerDown(canvas(), { button: 0, clientX: 0, clientY: 0 });
    fireEvent.pointerMove(window, { clientX: 30, clientY: 40 });
    fireEvent.pointerUp(window, { clientX: 30, clientY: 40 });
    expect(viewport()).toBe("translate(30 40) scale(1)");
    fireEvent.keyDown(canvas(), { key: "ArrowRight" });
    expect(viewport()).toBe("translate(-18 40) scale(1)");
    fireEvent.keyDown(canvas(), { key: "=", metaKey: true });
    expect(viewport()).toMatch(/scale\(1\.2\)$/);
    expect(within(dialog).getByText("120%")).toBeInTheDocument();
    fireEvent.keyDown(canvas(), { key: "-", metaKey: true });
    fireEvent.keyDown(canvas(), { key: "-", metaKey: true });
    expect(within(dialog).getByText("83%")).toBeInTheDocument();
    fireEvent.keyDown(canvas(), { key: "0", metaKey: true });
    expect(within(dialog).getByText("100%")).toBeInTheDocument();
  });

  it("⌘F searches, ⌘A selects everything, ⎋ closes", async () => {
    const user = userEvent.setup();
    const { dialog } = await setup();
    fireEvent.keyDown(canvas(), { key: "f", metaKey: true });
    const search = within(dialog).getByRole("textbox", { name: "Search tables" });
    expect(search).toHaveFocus();
    await user.type(search, "post_");
    // post_tags, post_counts, post_counts_mv match; the rest is dimmed.
    expect(card(dialog, "post_tags")).toHaveAttribute("opacity", "1");
    expect(card(dialog, "users")).toHaveAttribute("opacity", "0.32");
    await user.keyboard("{Enter}");
    expect(
      [...dialog.querySelectorAll<SVGGElement>("[data-selected]")].map((g) => g.dataset.table),
    ).toEqual(["post_counts", "post_counts_mv", "post_tags"]);
    // ⎋ in the search clears it first.
    await user.keyboard("{Escape}");
    expect(search).toHaveValue("");
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    fireEvent.keyDown(canvas(), { key: "a", metaKey: true });
    expect(dialog.querySelectorAll("[data-selected]")).toHaveLength(12);
    canvas().focus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(useDiagramStore.getState().request).toBeNull();
  });

  it("closes on ⌘W instead of closing the tab behind it", async () => {
    await setup();
    await act(() => actions.run("tab.close"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("exports SVG through the save panel", async () => {
    const user = userEvent.setup();
    const { dialog } = await setup();
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Downloads/public-diagram.svg");
    const write = vi.spyOn(ipc.files, "writeText");
    await user.click(within(dialog).getByRole("button", { name: /Export/ }));
    await user.click(await screen.findByRole("menuitem", { name: /SVG…/ }));
    await waitFor(() => expect(write).toHaveBeenCalledOnce());
    const [path, svg] = write.mock.calls[0]!;
    expect(path).toBe("/Users/me/Downloads/public-diagram.svg");
    expect(svg).toMatch(/^<\?xml/);
    expect(svg).toContain(">users<");
    expect(toast.success).toHaveBeenCalledWith("Diagram exported", {
      description: "/Users/me/Downloads/public-diagram.svg",
    });
  });

  it("is listed in the palette for the active connection's current schema", async () => {
    info = await ipc.connections.connect("p-local-pg");
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <DiagramHost />
      </QueryClientProvider>,
    );
    expect(actions.has("diagram.open")).toBe(false);
    act(() => {
      const store = useConnectionsStore.getState();
      store.add(info);
      store.setActive(info.connectionId);
      store.setCurrentSchema(info.connectionId, "inventory");
    });
    await waitFor(() => expect(actions.has("diagram.open")).toBe(true));
    await act(() => actions.run("diagram.open"));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Diagram — inventory")).toBeInTheDocument();
    await waitFor(() => expect(cards(dialog).sort()).toEqual(["items", "stock_movements"]));
  });
});
