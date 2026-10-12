import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { EMPTY_PINS } from "@/stores/pins";
import { useRecentStore } from "@/stores/recent";
import { useTabsStore } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";
import { EMPTY_RECENT } from "@/stores/recent";
import { RECENT_DELAY_MS, SchemaTree } from "./SchemaTree";
import { COLLAPSED_STORAGE_KEY, reloadSidebarPrefs, useSidebarPrefs } from "./sidebarPrefs";
import { buildTreeRows, useTreeRows } from "./useSchemaTree";

function table(name: string, kind: TableSummary["kind"] = "table"): TableSummary {
  return { schema: "public", name, kind, rowEstimate: null, sizeBytes: null, comment: null };
}

const tables = [table("users"), table("posts"), table("tags"), table("user_stats", "view")];
const SCOPE = "p|db";

function renderTree({ scope = null, recent = [] }: { scope?: string | null; recent?: string[] }) {
  const rows = buildTreeRows(tables, "", EMPTY_PINS, recent);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <SchemaTree
        connectionId="c1"
        rows={rows}
        loading={false}
        error={null}
        search=""
        pinScope={scope}
      />
    </QueryClientProvider>,
  );
}

/** The tree as the sidebar builds it: sections collapse through the sidebar preferences. */
function LiveTree({
  list = tables,
  showRowCounts,
}: {
  list?: TableSummary[];
  showRowCounts?: boolean;
}) {
  const collapsed = useSidebarPrefs((s) => s.collapsed);
  const rows = useTreeRows(list, "", EMPTY_PINS, EMPTY_RECENT, collapsed);
  return (
    <SchemaTree
      connectionId="c1"
      rows={rows}
      loading={false}
      error={null}
      search=""
      showRowCounts={showRowCounts}
    />
  );
}

function renderLiveTree(props: { list?: TableSummary[]; showRowCounts?: boolean } = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <LiveTree {...props} />
    </QueryClientProvider>,
  );
}

const header = (label: string) => screen.getByRole("button", { name: new RegExp(`^${label}`) });

/** The sidebar row of `name` (the last one: Recent rows come before the table's group). */
const row = (name: string) => screen.getAllByRole("button", { name }).at(-1)!;
const tabs = () => useTabsStore.getState().tabs;
/** `[title, preview]` of every open tab. */
const opened = () => tabs().map((t) => [t.title, t.kind !== "query" && t.preview === true]);
const activeTab = () => tabs().find((t) => t.id === useTabsStore.getState().activeId);
const recent = () => useRecentStore.getState().recent[SCOPE];

describe("SchemaTree", () => {
  // jsdom has no layout: give the scroller (and every row) a size so the virtualizer renders.
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(400);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(240);
    useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
    useTableTabsStore.setState({ slices: {} });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: "c1" });
    useRecentStore.setState({ recent: {} });
    localStorage.removeItem(COLLAPSED_STORAGE_KEY);
    reloadSidebarPrefs();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens a click in the preview tab, a double-click or middle-click in a kept tab", () => {
    renderTree({});
    fireEvent.click(row("users"));
    expect(tabs()).toEqual([expect.objectContaining({ title: "users", preview: true })]);
    fireEvent.click(row("posts"));
    expect(tabs()).toEqual([expect.objectContaining({ title: "posts", preview: true })]);

    fireEvent.doubleClick(row("posts"));
    expect(tabs()).toHaveLength(1);
    expect(tabs()[0]).not.toHaveProperty("preview");

    // Middle-click: no autoscroll, and a regular tab of its own.
    expect(fireEvent.mouseDown(row("tags"), { button: 1 })).toBe(false);
    fireEvent(row("tags"), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(opened()).toEqual([
      ["posts", false],
      ["tags", false],
    ]);
    expect(activeTab()?.title).toBe("tags");
  });

  it("↑/↓ move between tables and preview them while focus stays in the sidebar", () => {
    renderTree({ scope: SCOPE });
    row("users").focus();
    fireEvent.keyDown(row("users"), { key: "ArrowDown" });
    expect(row("posts")).toHaveFocus();
    expect(tabs()).toEqual([expect.objectContaining({ title: "posts", preview: true })]);

    fireEvent.keyDown(row("posts"), { key: "ArrowDown" });
    fireEvent.keyDown(row("tags"), { key: "ArrowDown" });
    // Over the "Views" header.
    expect(row("user_stats")).toHaveFocus();
    expect(tabs()).toEqual([expect.objectContaining({ title: "user_stats", preview: true })]);
    // Past the end: nothing moves.
    fireEvent.keyDown(row("user_stats"), { key: "ArrowDown" });
    expect(row("user_stats")).toHaveFocus();

    fireEvent.keyDown(row("user_stats"), { key: "ArrowUp" });
    expect(row("tags")).toHaveFocus();
    expect(tabs()).toEqual([expect.objectContaining({ title: "tags", preview: true })]);
    // Browsing is not recorded as recent.
    expect(recent()).toBeUndefined();

    // Enter keeps the tab (and records it).
    fireEvent.keyDown(row("tags"), { key: "Enter" });
    expect(tabs()).toHaveLength(1);
    expect(tabs()[0]).toMatchObject({ title: "tags" });
    expect(tabs()[0]).not.toHaveProperty("preview");
    expect(row("tags")).toHaveFocus();
    expect(recent()).toEqual(["public.tags"]);

    fireEvent.keyDown(row("tags"), { key: "ArrowUp" });
    expect(row("posts")).toHaveFocus();
    expect(opened()).toEqual([
      ["tags", false],
      ["posts", true],
    ]);
  });

  it("records mouse opens as recent once the double-click window has passed", async () => {
    renderTree({ scope: SCOPE });
    fireEvent.click(row("users"), { detail: 1 });
    fireEvent.click(row("posts"), { detail: 1 });
    expect(recent()).toBeUndefined();
    await waitFor(() => expect(recent()).toEqual(["public.posts", "public.users"]), {
      timeout: RECENT_DELAY_MS * 4,
    });
  });

  it("opens from the context menu: Open in New Tab, Open Structure", () => {
    renderTree({ scope: SCOPE, recent: ["public.tags"] });
    // The Recent row and the group row are both there.
    expect(screen.getAllByRole("button", { name: "tags" })).toHaveLength(2);

    fireEvent.click(row("users"));
    fireEvent.contextMenu(row("posts"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open in New Tab" }));
    expect(opened()).toEqual([
      ["users", true],
      ["posts", false],
    ]);

    fireEvent.contextMenu(screen.getAllByRole("button", { name: "tags" })[0]!);
    fireEvent.click(screen.getByRole("menuitem", { name: "Open Structure" }));
    expect(activeTab()).toMatchObject({ title: "tags", kind: "structure" });
    expect(activeTab()).not.toHaveProperty("preview");
    expect(recent()).toEqual(["public.tags", "public.posts", "public.users"]);
  });

  it("collapses a section from its header (remembered), and ↑/↓ skip its rows", () => {
    renderLiveTree();
    expect(header("Tables")).toHaveAttribute("aria-expanded", "true");
    expect(header("Tables")).toHaveTextContent("Tables3");
    fireEvent.click(header("Tables"));
    expect(header("Tables")).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "users" })).toBeNull();
    expect(JSON.parse(localStorage.getItem(COLLAPSED_STORAGE_KEY)!)).toEqual(["Tables"]);

    // ↓ from the collapsed header goes to the next listed table (and previews it).
    header("Tables").focus();
    fireEvent.keyDown(header("Tables"), { key: "ArrowDown" });
    expect(row("user_stats")).toHaveFocus();
    expect(tabs()).toEqual([expect.objectContaining({ title: "user_stats", preview: true })]);
    // Nothing above it but the collapsed section.
    fireEvent.keyDown(row("user_stats"), { key: "ArrowUp" });
    expect(row("user_stats")).toHaveFocus();

    // → expands, ← collapses.
    fireEvent.keyDown(header("Tables"), { key: "ArrowRight" });
    expect(header("Tables")).toHaveAttribute("aria-expanded", "true");
    expect(row("tags")).toBeInTheDocument();
    fireEvent.keyDown(row("user_stats"), { key: "ArrowUp" });
    expect(row("tags")).toHaveFocus();
    fireEvent.keyDown(header("Views"), { key: "ArrowLeft" });
    expect(screen.queryByRole("button", { name: "user_stats" })).toBeNull();
    expect(useSidebarPrefs.getState().collapsed).toEqual(new Set(["Views"]));
  });

  it("shows row estimates only when they are turned on", () => {
    const list = [{ ...table("users"), rowEstimate: 1234 }, table("posts")];
    const users = () => screen.getByRole("button", { name: /^users/ });
    const { unmount } = renderLiveTree({ list });
    expect(users()).toHaveTextContent(/^users$/);
    unmount();
    renderLiveTree({ list, showRowCounts: true });
    expect(users()).toHaveTextContent("~1,234");
  });

  it("fills only the open table's row; keyboard focus is a ring, not a second fill", () => {
    renderTree({});
    fireEvent.click(row("users"));
    expect(row("users")).toHaveAttribute("data-active", "true");
    expect(row("users")).toHaveClass("bg-sidebar-accent");

    // Focus moves without opening (↓ from the search field): no fill, a focus-visible ring.
    act(() => row("posts").focus());
    expect(row("posts")).toHaveFocus();
    expect(row("posts")).not.toHaveAttribute("data-active");
    const classes = row("posts").className.split(/\s+/);
    expect(classes.filter((c) => c.includes("bg-sidebar-accent"))).toEqual([]);
    expect(classes).toContain("focus-visible:ring-1");
    expect(classes.some((c) => c.startsWith("focus-visible:bg-"))).toBe(false);
    expect(header("Tables").className).not.toMatch(/focus-visible:bg-/);
    // Only one row is filled.
    expect(document.querySelectorAll("[data-row-index].bg-sidebar-accent")).toHaveLength(1);
  });

  it("follows the collapsed state when it changes elsewhere", () => {
    renderLiveTree();
    act(() => useSidebarPrefs.setState({ collapsed: new Set(["Views"]) }));
    expect(header("Views")).toHaveAttribute("aria-expanded", "false");
    expect(row("users")).toBeInTheDocument();
  });
});
