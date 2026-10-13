import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type CsvPreview, type CsvPreviewRequest } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { ImportHost } from "./ImportHost";
import { openImport, useImportStore } from "./store";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), message: vi.fn(), warning: vi.fn() },
}));

let info: ConnectionInfo;

async function openWizard(file: string) {
  info = await ipc.connections.connect("p-local-pg");
  const store = useConnectionsStore.getState();
  store.add(info);
  store.setActive(info.connectionId);
  store.setCurrentSchema(info.connectionId, "public");
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ImportHost />
    </QueryClientProvider>,
  );
  act(() =>
    openImport(
      { connectionId: info.connectionId, kind: "csv", table: { schema: "public", name: "users" } },
      file,
    ),
  );
  await screen.findByRole("dialog");
}

/** A preview the detection could not split: one column whatever the delimiter. */
function oneColumn(req: CsvPreviewRequest): CsvPreview {
  return {
    path: req.path,
    fileName: "notes.csv",
    size: 20,
    delimiter: req.format.delimiter ?? ",",
    encoding: "utf8",
    columns: [
      { name: "note", detected: { name: "note", ty: "text", precision: null, scale: null } },
    ],
    rows: [["a;b"]],
    sampledRows: 1,
    more: false,
  };
}

beforeEach(() => {
  localStorage.clear();
  useImportStore.setState({ request: null, path: null, session: 0 });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
});

afterEach(() => {
  vi.restoreAllMocks();
  resetPrefsForTests();
});

describe("CSV import wizard: CSV default options", () => {
  it("keeps a detected delimiter that splits the file", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";" });
    const preview = vi.spyOn(ipc.imports, "previewCsv");
    await openWizard("/Users/me/Downloads/users.csv");
    await waitFor(() => expect(preview.mock.calls.length).toBeGreaterThan(0));
    await screen.findByText("user0@example.com");
    // The resolved options reuse the first preview: no second read with another delimiter.
    expect(preview.mock.calls.every(([r]) => r.format.delimiter === null)).toBe(true);
    expect(screen.getByRole("combobox", { name: "Delimiter" })).toHaveTextContent("Comma");
  });

  it("falls back to the preferred delimiter and quote when the detection is not sure", async () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, csvDelimiter: ";", csvQuote: "single" });
    const preview = vi
      .spyOn(ipc.imports, "previewCsv")
      .mockImplementation(async (req) => oneColumn(req));
    await openWizard("/Users/me/Downloads/notes.csv");
    await waitFor(() =>
      expect(preview).toHaveBeenLastCalledWith(
        expect.objectContaining({
          format: expect.objectContaining({ delimiter: ";", quote: "'" }) as unknown,
        }),
      ),
    );
    expect(preview.mock.calls[0]![0].format).toMatchObject({ delimiter: null, quote: "'" });
  });
});
