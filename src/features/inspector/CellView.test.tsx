import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
import { sortKeysStorageKey } from "@/features/datagrid/quicklook/jsonPrefs";
import type { Value } from "@/lib/ipc";
import type { InspectedCell } from "@/stores/inspector";
import { CellView } from "./CellView";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

const col = (kind: GridColumn["kind"], name = "c"): GridColumn => ({
  id: name,
  name,
  kind,
  dataType: kind,
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
});
const bytes = (data: number[], truncated = false, len = data.length): Value => ({
  t: "bytes",
  v: { base64: btoa(String.fromCharCode(...data)), len, truncated },
});
const utf8 = (text: string) => Array.from(new TextEncoder().encode(text));

function cell(column: GridColumn, value: Value): InspectedCell {
  return { column, value, rowIndex: 0, source: "t", fetch: null };
}

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("CellView: a blob that is text", () => {
  it("reads as text by default, with a Hex / Text toggle for the dump", () => {
    render(<CellView cell={cell(col("bytes"), bytes(utf8("Héllo wörld\nline 2")))} />);
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual(["Hex", "Text"]);
    expect(screen.getByRole("radio", { name: "Text" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("cell-view-text").textContent).toBe("Héllo wörld\nline 2");
    expect(screen.queryByTestId("cell-view-hex")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Hex" }));
    expect(screen.getByTestId("cell-view-hex")).toHaveTextContent("48 c3 a9 6c 6c 6f");
    expect(screen.queryByTestId("cell-view-text")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "Text" }));
    expect(screen.getByTestId("cell-view-text")).toHaveTextContent("Héllo wörld");
  });

  it("offers no Text for binary data: a hex dump only", () => {
    render(<CellView cell={cell(col("bytes"), bytes([0x1f, 0x8b, 0x08, 0x00, 0xfe, 0x80]))} />);
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.getByTestId("cell-view-hex")).toHaveTextContent("1f 8b 08 00 fe 80");
  });

  it("reads a preview as text only while the preview itself is valid text", () => {
    // Cut in the middle of 日: the whole characters before it are the text.
    const cut = utf8("ab日").slice(0, 4);
    render(<CellView cell={cell(col("bytes"), bytes(cut, true, 70_000))} />);
    expect(screen.getByTestId("cell-view-text").textContent).toBe("ab");
    expect(screen.getByText(/\(preview\)/)).toBeInTheDocument();
    cleanup();
    render(<CellView cell={cell(col("bytes"), bytes([0x61, 0xff, 0x62], true, 70_000))} />);
    expect(screen.queryByRole("radio", { name: "Text" })).not.toBeInTheDocument();
  });
});

describe("CellView: JSON with sorted keys", () => {
  const raw = '{"b":9007199254740993,"a":{"d":1.10,"c":"\\u00e9"},"b":2}';

  it("sorts the keys for display only, keeps every literal, and remembers the choice", () => {
    render(<CellView cell={cell(col("json"), { t: "json", v: raw })} />);
    const pre = () => document.querySelector("pre")!.textContent;
    expect(pre()).toBe(
      '{\n  "b": 9007199254740993,\n  "a": {\n    "d": 1.10,\n    "c": "\\u00e9"\n  },\n  "b": 2\n}',
    );
    const toggle = screen.getByRole("button", { name: "Sort keys" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(pre()).toBe(
      '{\n  "a": {\n    "c": "\\u00e9",\n    "d": 1.10\n  },\n  "b": 9007199254740993,\n  "b": 2\n}',
    );
    expect(localStorage.getItem(sortKeysStorageKey("cell"))).toBe("1");
    // A later Cell view opens sorted; Copy still hands out the value as stored.
    cleanup();
    render(<CellView cell={cell(col("json"), { t: "json", v: raw })} />);
    expect(screen.getByRole("button", { name: "Sort keys" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(pre()?.indexOf('"a"')).toBeLessThan(pre()?.indexOf('"b"') ?? -1);
    fireEvent.click(screen.getByRole("button", { name: "Sort keys" }));
    expect(pre()?.indexOf('"b"')).toBeLessThan(pre()?.indexOf('"a"') ?? -1);
  });

  it("has no toggle for non-JSON values or JSON that does not parse", () => {
    render(<CellView cell={cell(col("text"), { t: "text", v: "{}" })} />);
    expect(screen.queryByRole("button", { name: "Sort keys" })).not.toBeInTheDocument();
    cleanup();
    render(<CellView cell={cell(col("json"), { t: "json", v: "{oops" })} />);
    expect(screen.queryByRole("button", { name: "Sort keys" })).not.toBeInTheDocument();
    expect(document.querySelector("pre")!.textContent).toBe("{oops");
  });

  it("works when localStorage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    render(<CellView cell={cell(col("json"), { t: "json", v: '{"b":1,"a":2}' })} />);
    fireEvent.click(screen.getByRole("button", { name: "Sort keys" }));
    expect(document.querySelector("pre")!.textContent).toBe('{\n  "a": 2,\n  "b": 1\n}');
  });
});
