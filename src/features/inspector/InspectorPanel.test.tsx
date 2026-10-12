import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
import { ipc, type Value } from "@/lib/ipc";
import { useInspectorStore, type InspectedCell } from "@/stores/inspector";
import { InspectorPanel } from "./InspectorPanel";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

const col = (id: string, kind: GridColumn["kind"]): GridColumn => ({
  id,
  name: id,
  kind,
  dataType: kind === "bytes" ? "bytea" : kind,
  nullable: true,
  isPk: id === "id",
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});
const columns = [col("id", "int"), col("bio", "text"), col("avatar", "bytes"), col("note", "text")];
const preview: Value = { t: "bytes", v: { base64: btoa("Hel"), len: 70_000, truncated: true } };
const cells: Value[] = [
  { t: "int", v: 1 },
  { t: "text", v: "line 1\nline 2" },
  preview,
  { t: "null" },
];

function show(cell: Partial<InspectedCell> & { focused: number }) {
  const { focused, ...rest } = cell;
  act(() =>
    useInspectorStore.setState({
      cell: {
        column: columns[focused]!,
        value: cells[focused]!,
        rowIndex: 0,
        source: "users",
        row: { columns, cells, focused, hidden: ["note"] },
        fetch: {
          connectionId: "c1",
          table: { schema: "public", name: "users" },
          key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
          column: columns[focused]!.name,
        },
        ...rest,
      },
    }),
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  useInspectorStore.setState({ cell: null, mode: "cell" });
});

describe("InspectorPanel", () => {
  it("shows text stats in the Cell view", () => {
    render(<InspectorPanel />);
    show({ focused: 1 });
    expect(screen.getByText("· 13 chars · 2 lines")).toBeInTheDocument();
  });

  it("loads the full blob on demand", async () => {
    const full = new Uint8Array(70_000).fill(65);
    let binary = "";
    for (const b of full) binary += String.fromCharCode(b);
    const cell = vi.spyOn(ipc.browse, "cell").mockResolvedValue({
      t: "bytes",
      v: { base64: btoa(binary), len: 70_000, truncated: false },
    });
    render(<InspectorPanel />);
    show({ focused: 2 });
    expect(screen.getByText(/\(preview\)/)).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Load full value/ })));
    expect(cell).toHaveBeenCalledWith(
      "c1",
      { schema: "public", name: "users" },
      {
        kind: "pk",
        value: { id: { t: "int", v: 1 } },
      },
      "avatar",
    );
    expect(screen.queryByRole("button", { name: /Load full value/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/\(preview\)/)).not.toBeInTheDocument();
    expect(screen.getByText(/Showing the first 4\.0 KB of 68\.4 KB/)).toBeInTheDocument();
  });

  it("never presents a still-truncated result as the full value", async () => {
    vi.spyOn(ipc.browse, "cell").mockResolvedValue(preview);
    render(<InspectorPanel />);
    show({ focused: 2 });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /Load full value/ })));
    expect(screen.getByText(/Only a preview of this value could be loaded/)).toBeInTheDocument();
    expect(screen.getByText(/\(preview\)/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Load full value/ })).toBeInTheDocument();
  });

  it("shows and copies JSON exactly as stored (no JSON.parse round trip)", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>().mockResolvedValue();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText, readText: vi.fn() },
    });
    const raw = '{"id": 9007199254740993, "p": 1.10, "a": 1, "a": 2, "u": "\\u00e9"}';
    const json: Value = { t: "json", v: raw };
    const doc = col("doc", "json");
    const pretty =
      '{\n  "id": 9007199254740993,\n  "p": 1.10,\n  "a": 1,\n  "a": 2,\n  "u": "\\u00e9"\n}';
    const isPretty = (_: string, el: Element | null) =>
      el?.tagName === "PRE" && el.textContent === pretty;

    render(<InspectorPanel />);
    show({
      focused: 0,
      column: doc,
      value: json,
      row: { columns: [doc], cells: [json], focused: 0 },
    });
    expect(screen.getByText(isPretty)).toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy value" })));
    expect(writeText).toHaveBeenLastCalledWith(raw);

    act(() => useInspectorStore.setState({ mode: "row" }));
    const list = screen.getByRole("list", { name: "Row fields" });
    fireEvent.click(within(list).getByRole("button", { expanded: false }));
    expect(within(list).getByText(isPretty)).toBeInTheDocument();
    await act(async () => fireEvent.click(within(list).getByRole("button", { name: "Copy doc" })));
    expect(writeText).toHaveBeenLastCalledWith(raw);
  });

  it("offers no full load without a fetch target", () => {
    render(<InspectorPanel />);
    show({ focused: 2, fetch: null });
    expect(screen.queryByRole("button", { name: /Load full value/ })).not.toBeInTheDocument();
  });

  it("lists the whole row in the Row view, filtered and expandable", () => {
    useInspectorStore.setState({ mode: "row" });
    render(<InspectorPanel />);
    show({ focused: 1 });
    const list = screen.getByRole("list", { name: "Row fields" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(items[1]).toHaveClass("bg-brand-soft");
    expect(within(items[3]!).getByText("hidden")).toBeInTheDocument();
    expect(within(items[3]!).getByText("NULL")).toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "Filter fields" }), {
      target: { value: "BI" },
    });
    const filtered = within(list).getAllByRole("listitem");
    expect(filtered).toHaveLength(1);
    fireEvent.click(within(filtered[0]!).getByRole("button", { expanded: false }));
    expect(
      within(list).getByText(
        (_, el) => el?.tagName === "PRE" && el.textContent === "line 1\nline 2",
      ),
    ).toBeInTheDocument();
  });
});
