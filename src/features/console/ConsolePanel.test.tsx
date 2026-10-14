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

/** The `<pre>` showing exactly `sql` (highlighting splits it into spans). */
const sqlBlock = (sql: string) =>
  screen.getByText((_, el) => el?.tagName === "PRE" && el.textContent === sql);

afterEach(() => {
  vi.restoreAllMocks();
  useConsoleStore.setState({ entries: [], filter: "all" });
  useUiStore.setState({ consoleOpen: false });
});

describe("ConsolePanel", () => {
  it("shows an empty state until something is logged", () => {
    render(<ConsolePanel />);
    expect(screen.getByText("Nothing logged yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Clear console" })).toBeDisabled();
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
    expect(
      first.getByText(
        (_, el) => el?.tagName === "PRE" && el.textContent === 'SELECT * FROM "users" LIMIT 5',
      ),
    ).toBeInTheDocument();
    expect(first.getByText("4 ms · 5 rows")).toBeInTheDocument();
    expect(within(rows[1]!).getByText('relation "nosuch" does not exist')).toHaveClass(
      "text-error",
    );
    expect(within(rows[2]!).getByText("-- list tables in public")).toHaveClass("italic");
    expect(screen.getByText("3 entries")).toBeInTheDocument();
  });

  it("filters Meta / Data / All queries (TablePlus's labels) and clears", () => {
    render(<ConsolePanel />);
    seed([
      entry({ id: 1, sql: "SELECT 1" }),
      entry({ id: 2, category: "query", sql: "select 2" }),
      entry({ id: 3, category: "structure", sql: "-- list tables in public", meta: true }),
    ]);
    expect(screen.getByRole("combobox", { name: "Console filter" })).toHaveTextContent(
      "All queries",
    );
    act(() => useConsoleStore.getState().setFilter("meta"));
    expect(screen.getByRole("combobox", { name: "Console filter" })).toHaveTextContent(
      "Meta queries only",
    );
    expect(screen.getAllByTestId("console-entry")).toHaveLength(1);
    expect(screen.getByText("-- list tables in public")).toBeInTheDocument();
    act(() => useConsoleStore.getState().setFilter("data"));
    expect(screen.getAllByTestId("console-entry")).toHaveLength(2);
    expect(sqlBlock("select 2")).toBeInTheDocument();
    act(() =>
      useConsoleStore.setState({ entries: [entry({ id: 4, category: "structure", sql: "x" })] }),
    );
    expect(screen.getByText("No data queries")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear console" }));
    expect(useConsoleStore.getState().entries).toEqual([]);
    expect(screen.getByText("Nothing logged yet")).toBeInTheDocument();
  });

  it("highlights the SQL with the editor's colours, meta descriptions stay plain", () => {
    render(<ConsolePanel />);
    seed([
      entry({ id: 1, sql: "SELECT 'a', 42 FROM t -- note", dialect: "postgres" }),
      entry({ id: 2, category: "structure", sql: "-- list schemas", meta: true }),
    ]);
    const [sql, meta] = screen.getAllByTestId("console-entry");
    const pre = sql!.querySelector("pre")!;
    expect(pre.textContent).toBe("SELECT 'a', 42 FROM t -- note");
    const classes = (text: string) =>
      Array.from(pre.querySelectorAll("span")).find((s) => s.textContent === text)?.className;
    expect(classes("SELECT")).toContain("--syn-keyword");
    expect(classes("'a'")).toContain("--syn-string");
    expect(classes("42")).toContain("--syn-number");
    expect(classes("-- note")).toContain("--syn-comment");
    expect(meta!.querySelector("pre span")).toBeNull();
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
