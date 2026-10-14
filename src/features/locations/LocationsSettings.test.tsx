import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockLocations } from "@/lib/ipc/mock/locations";
import { resetPrefsForTests, setPref, usePrefsStore } from "@/stores/prefs";
import { LocationsHost } from "./LocationsHost";
import { LocationsSettings } from "./LocationsSettings";
import { locationPrompt } from "./model";
import { resetLocationsStore } from "./store";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));

function renderWith(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
  return qc;
}

beforeEach(() => {
  resetMockLocations();
  resetLocationsStore();
  resetPrefsForTests();
  Object.values(toast).forEach((f) => f.mockClear());
});
afterEach(() => vi.restoreAllMocks());

describe("locationPrompt", () => {
  const seen = {
    path: "/x/Tabler",
    isCurrent: false,
    hasData: false,
    queryFiles: 0,
    problem: null,
  };
  it("offers Move existing data / Start empty, or Use / Replace when data is there", () => {
    expect(locationPrompt("data", seen).choices.map((c) => c.change)).toEqual(["empty", "move"]);
    expect(locationPrompt("data", { ...seen, hasData: true }).choices.map((c) => c.change)).toEqual(
      ["replace", "useExisting"],
    );
    const q = locationPrompt("queries", { ...seen, hasData: true, queryFiles: 3 });
    expect(q.title).toContain("3 .sql files");
    expect(q.choices.map((c) => c.change)).toEqual(["useExisting", "move"]);
  });
});

describe("Settings ▸ Locations", () => {
  it("moves the connections & settings folder after a restart, and queries at once", async () => {
    const setData = vi.spyOn(ipc.locations, "setData");
    const setQueries = vi.spyOn(ipc.locations, "setQueries");
    renderWith(<LocationsSettings />);
    expect(await screen.findByText(/tabler\.sqlite/)).toBeInTheDocument();
    expect(screen.getByText(/stay in the macOS Keychain/)).toBeInTheDocument();

    vi.spyOn(window, "prompt").mockReturnValueOnce("/Users/me/Dropbox/Tabler");
    fireEvent.click(screen.getAllByRole("button", { name: "Choose…" })[0]!);
    fireEvent.click(await screen.findByRole("button", { name: "Move Existing Data" }));
    await waitFor(() => expect(setData).toHaveBeenCalledWith("/Users/me/Dropbox/Tabler", "move"));
    expect(
      await screen.findByText("After a restart: /Users/me/Dropbox/Tabler"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restart Now" })).toBeInTheDocument();

    // A folder that already holds data: Use Its Data / Replace It.
    vi.spyOn(window, "prompt").mockReturnValueOnce(
      "/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Tabler",
    );
    fireEvent.click(screen.getAllByRole("button", { name: "Choose…" })[0]!);
    expect(await screen.findByRole("button", { name: "Replace It" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    vi.spyOn(window, "prompt").mockReturnValueOnce("/Users/me/queries");
    fireEvent.click(screen.getAllByRole("button", { name: "Choose…" })[1]!);
    fireEvent.click(await screen.findByRole("button", { name: "Move Existing Queries" }));
    await waitFor(() => expect(setQueries).toHaveBeenCalledWith("/Users/me/queries", "move"));
    expect(await screen.findByText("/Users/me/queries")).toBeInTheDocument();
  });

  it("refuses folders that do not exist", async () => {
    renderWith(<LocationsSettings />);
    await screen.findByText(/tabler\.sqlite/);
    vi.spyOn(window, "prompt").mockReturnValueOnce("/Volumes/Gone");
    fireEvent.click(screen.getAllByRole("button", { name: "Choose…" })[0]!);
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("That folder cannot be used", {
        description: "The folder does not exist.",
      }),
    );
  });
});

describe("LocationsHost", () => {
  it("shows the startup warnings and prunes old history", async () => {
    resetMockLocations({ warnings: ["The queries folder /Volumes/X is not available."] });
    const clearBefore = vi.spyOn(ipc.history, "clearBefore");
    setPref("historyRetention", "1y");
    usePrefsStore.setState({ loaded: true });
    renderWith(<LocationsHost />);
    await waitFor(() =>
      expect(toast.warning).toHaveBeenCalledWith(
        "The queries folder /Volumes/X is not available.",
        expect.any(Object),
      ),
    );
    await waitFor(() => expect(clearBefore).toHaveBeenCalledOnce());
    const cutoff = new Date(clearBefore.mock.calls[0]![0]);
    expect(Date.now() - cutoff.getTime()).toBeGreaterThan(364 * 86_400_000);
  });
});
