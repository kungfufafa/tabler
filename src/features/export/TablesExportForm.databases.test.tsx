import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { actions } from "@/features/palette/actions";
import { ipc, type ConnectionInfo, type TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { useTabsStore } from "@/stores/tabs";
import { NotifyHost } from "@/features/notify/NotifyHost";
import { ExportHost } from "./ExportHost";
import { openExportTables, resetExportStore } from "./store";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const table = (name: string): TableSummary => ({
  name,
  schema: "public",
  kind: "table",
  rowEstimate: null,
  sizeBytes: null,
  comment: null,
});

const database = (name: string, isCurrent: boolean) => ({
  name,
  owner: null,
  encoding: null,
  isCurrent,
});

let info: ConnectionInfo;

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  localStorage.clear();
  resetPrefsForTests({ ...DEFAULT_PREFS, exportRevealPopup: false });
  resetExportStore();
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useTabsStore.setState({ tabs: [], activeId: null, closed: [], closeRequest: null });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

/** The export dialog of a connection whose databases are `names` (the first is the current one). */
async function openWith(names: string[], tables: Record<string, string>) {
  info = await ipc.connections.connect("p-local-pg");
  useConnectionsStore.getState().add(info);
  vi.spyOn(ipc.schema, "databases").mockResolvedValue(names.map((n, i) => database(n, i === 0)));
  vi.spyOn(ipc.exports, "databaseTables").mockImplementation(async (_c, db) => [
    table(tables[db ?? names[0]!] ?? "t"),
  ]);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ExportHost />
      <NotifyHost />
    </QueryClientProvider>,
  );
  await waitFor(() => expect(actions.has("export.open")).toBe(true));
  act(() => openExportTables(info.connectionId, [{ schema: "public", name: tables[names[0]!]! }]));
  return screen.findByRole("dialog");
}

async function checkAllOf(
  user: ReturnType<typeof userEvent.setup>,
  dialog: HTMLElement,
  db: string,
) {
  await user.click(
    await within(dialog).findByRole("checkbox", { name: `Select all tables of ${db}` }),
  );
  await waitFor(() =>
    expect(
      within(dialog).getByRole("checkbox", { name: `Select all tables of ${db}` }),
    ).toHaveAttribute("aria-checked", "true"),
  );
}

describe("exporting several databases whose file names collide", () => {
  const exportButton = (dialog: HTMLElement) =>
    within(dialog).getByRole("button", { name: /^Export…/ });

  it("gives each database its own folder", async () => {
    const user = userEvent.setup();
    // The connection's current database is tabler_dev; the others reduce to the same name.
    const dialog = await openWith(["tabler_dev", "tabler dev", "Tabler_Dev"], {
      tabler_dev: "a",
      "tabler dev": "b",
      Tabler_Dev: "c",
    });
    await checkAllOf(user, dialog, "tabler dev");
    await checkAllOf(user, dialog, "Tabler_Dev");
    // CSV: one file per table, a folder per database.
    await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledTimes(3);
    const targets = run.mock.calls.map((c) => c[1].target);
    expect(targets).toEqual([
      "/Users/me/Exports/tabler_dev",
      "/Users/me/Exports/tabler_dev_2",
      "/Users/me/Exports/Tabler_Dev_3",
    ]);
    expect(new Set(targets.map((t) => t.toLowerCase())).size).toBe(3);
    expect(run.mock.calls.map((c) => c[1].database)).toEqual([null, "tabler dev", "Tabler_Dev"]);
  });

  it("gives each database its own file when everything goes into one file", async () => {
    const user = userEvent.setup();
    const dialog = await openWith(["tabler_dev", "$$", "@@"], {
      tabler_dev: "a",
      $$: "b",
      "@@": "c",
    });
    await checkAllOf(user, dialog, "$$");
    await checkAllOf(user, dialog, "@@");
    await user.click(within(dialog).getByRole("radio", { name: "SQL" }));
    await user.click(within(dialog).getByRole("radio", { name: "One file" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    const targets = run.mock.calls.map((c) => c[1].target);
    expect(targets).toHaveLength(3);
    expect(targets[0]).toMatch(/^\/Users\/me\/Exports\/tabler_dev_\d[\w-]*\.sql$/);
    expect(targets[1]).toMatch(/^\/Users\/me\/Exports\/database_\d[\w-]*\.sql$/);
    expect(targets[2]).toMatch(/^\/Users\/me\/Exports\/database_2_\d[\w-]*\.sql$/);
    expect(new Set(targets).size).toBe(3);
  });

  it("keeps the names of databases that do not collide", async () => {
    const user = userEvent.setup();
    const dialog = await openWith(["tabler_dev", "blog"], { tabler_dev: "a", blog: "b" });
    await checkAllOf(user, dialog, "blog");
    await user.click(within(dialog).getByRole("radio", { name: "CSV" }));
    vi.spyOn(window, "prompt").mockReturnValue("/Users/me/Exports");
    const run = vi.spyOn(ipc.exports, "runTables");
    await user.click(exportButton(dialog));
    await waitFor(() => expect(toast.success).toHaveBeenCalledOnce());
    expect(run.mock.calls.map((c) => c[1].target)).toEqual([
      "/Users/me/Exports/tabler_dev",
      "/Users/me/Exports/blog",
    ]);
  });
});
