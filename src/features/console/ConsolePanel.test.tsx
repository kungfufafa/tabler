import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useUiStore } from "@/stores/ui";
import { ConsolePanel } from "./ConsolePanel";
import { useConsoleStore, type ConsoleEntry } from "./store";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() } }));

const AT = new Date(2026, 9, 12, 9, 5, 7).getTime();

function entry(over: Partial<ConsoleEntry> & Pick<ConsoleEntry, "id" | "sql">): ConsoleEntry {
  return {
    at: AT,
    connectionId: "c1",
    connectionName: "Local Postgres",
    database: "tabler_dev",
    category: "data",
    meta: false,
    ...over,
  };
}

function seed(entries: ConsoleEntry[]) {
  act(() => useConsoleStore.setState({ entries }));
}

afterEach(() => {
  vi.restoreAllMocks();
  useConsoleStore.setState({ entries: [], filter: "all" });
  useUiStore.setState({ consoleOpen: false });
});

describe("ConsolePanel", () => {
  it("shows an empty state until something is logged", () => {
    render(<ConsolePanel />);
    expect(screen.getByText("Nothing logged yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Clear/ })).toBeDisabled();
  });

  it("lists entries with time, category, source, SQL and stats", () => {
    render(<ConsolePanel />);
    seed([
      entry({ id: 1, sql: 'SELECT * FROM "users" LIMIT 5', durationMs: 4, rows: 5 }),
      entry({
        id: 2,
        category: "query",
        sql: "select * from nosuch",
        error: 'relation "nosuch" does not exist',
      }),
      entry({ id: 3, category: "structure", sql: "-- list tables in public", meta: true }),
    ]);
    const rows = screen.getAllByTestId("console-entry");
    expect(rows).toHaveLength(3);
    const first = within(rows[0]!);
    expect(first.getByText("09:05:07")).toBeInTheDocument();
    expect(first.getByText("Data")).toBeInTheDocument();
    expect(first.getByText("Local Postgres · tabler_dev")).toBeInTheDocument();
    expect(first.getByText('SELECT * FROM "users" LIMIT 5')).toBeInTheDocument();
    expect(first.getByText("4 ms · 5 rows")).toBeInTheDocument();
    expect(within(rows[1]!).getByText('relation "nosuch" does not exist')).toHaveClass(
      "text-error",
    );
    expect(within(rows[2]!).getByText("-- list tables in public")).toHaveClass("italic");
    expect(screen.getByText("3 entries")).toBeInTheDocument();
  });

  it("filters by category and clears", () => {
    render(<ConsolePanel />);
    seed([entry({ id: 1, sql: "SELECT 1" }), entry({ id: 2, category: "query", sql: "select 2" })]);
    fireEvent.click(screen.getByRole("radio", { name: "Query" }));
    expect(screen.getByRole("radio", { name: "Query" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByText("SELECT 1")).not.toBeInTheDocument();
    expect(screen.getByText("select 2")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "Structure" }));
    expect(screen.getByText("No structure entries")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Clear/ }));
    expect(useConsoleStore.getState().entries).toEqual([]);
    expect(screen.getByText("Nothing logged yet")).toBeInTheDocument();
  });

  it("collapses long SQL behind Show more", () => {
    render(<ConsolePanel />);
    const sql = Array.from({ length: 10 }, (_, i) => `SELECT ${i};`).join("\n");
    seed([entry({ id: 1, sql })]);
    const pre = screen.getByText((_, el) => el?.tagName === "PRE" && el.textContent === sql);
    expect(pre).toHaveClass("line-clamp-6");
    fireEvent.click(screen.getByRole("button", { name: "Show more" }));
    expect(pre).not.toHaveClass("line-clamp-6");
    expect(screen.getByRole("button", { name: "Show less" })).toBeInTheDocument();
  });

  it("copies an entry's SQL", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ConsolePanel />);
    seed([entry({ id: 1, sql: "SELECT 42" })]);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy SQL" })));
    expect(writeText).toHaveBeenCalledWith("SELECT 42");
  });

  it("closes through the ui store", () => {
    useUiStore.setState({ consoleOpen: true });
    render(<ConsolePanel />);
    fireEvent.click(screen.getByRole("button", { name: "Close console" }));
    expect(useUiStore.getState().consoleOpen).toBe(false);
  });
});
