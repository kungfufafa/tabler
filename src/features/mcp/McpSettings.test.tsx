import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockMcp } from "@/lib/ipc/mock/mcp";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { McpSettings } from "./McpSettings";

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
}));
vi.mock("sonner", () => ({ toast }));
const clipboard = vi.hoisted(() => ({ writeClipboard: vi.fn(async () => {}) }));
vi.mock("@/features/datagrid/clipboard", () => clipboard);
const platform = vi.hoisted(() => ({ windows: false }));
vi.mock("@/lib/platform", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/platform")>()),
  get isWindows() {
    return platform.windows;
  },
}));

const EXE = "/Applications/Tabler.app/Contents/MacOS/tabler";

function renderSettings() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <McpSettings />
    </QueryClientProvider>,
  );
}

/** The first IPC call loads the mock backend, which can be slow on a busy machine. */
const commandShown = () => screen.findByText(`${EXE} --mcp`, {}, { timeout: 5000 });

const installButton = (name: RegExp) =>
  within(screen.getByRole("group", { name: "Install in" })).getByRole("button", { name });

beforeEach(() => {
  platform.windows = false;
  resetMockMcp();
  resetPrefsForTests();
  Object.values(toast).forEach((f) => f.mockClear());
  clipboard.writeClipboard.mockClear();
});
afterEach(() => vi.restoreAllMocks());

describe("Settings ▸ MCP Server", () => {
  it("is off and read-only by default, and shows the command to register", async () => {
    renderSettings();
    expect(await commandShown()).toBeInTheDocument();
    const enable = screen.getByRole("switch", { name: /Enable MCP server/ });
    expect(enable).not.toBeChecked();
    expect(screen.getByRole("switch", { name: /Allow MCP clients to change data/ })).toBeDisabled();
    expect(screen.getByRole("switch", { name: /Expose production connections/ })).toBeDisabled();
    expect(installButton(/Claude Code/)).toBeDisabled();

    fireEvent.click(enable);
    expect(getPrefs().mcpEnabled).toBe(true);
    expect(getPrefs().mcpAllowWrites).toBe(false);
    expect(getPrefs().mcpExposeProduction).toBe(false);
    const writes = screen.getByRole("switch", { name: /Allow MCP clients to change data/ });
    expect(writes).toBeEnabled();
    fireEvent.click(writes);
    expect(getPrefs().mcpAllowWrites).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: /Copy/ }));
    await waitFor(() => expect(clipboard.writeClipboard).toHaveBeenCalledWith(`${EXE} --mcp`));
  });

  it("installs through the Claude Code CLI, and copies the Codex command without its CLI", async () => {
    setPref("mcpEnabled", true);
    renderSettings();
    await commandShown();
    fireEvent.click(installButton(/Claude Code/));
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith("Added Tabler to Claude Code", expect.anything()),
    );
    expect(clipboard.writeClipboard).not.toHaveBeenCalled();

    fireEvent.click(installButton(/Codex/));
    await waitFor(() =>
      expect(clipboard.writeClipboard).toHaveBeenCalledWith(`codex mcp add tabler -- ${EXE} --mcp`),
    );
    expect(toast.message).toHaveBeenCalledWith(
      "Codex's command line tool was not found",
      expect.objectContaining({ description: expect.stringContaining("paste it in a terminal") }),
    );
  });

  it("labels the Windows launch command and copied registration command for PowerShell", async () => {
    platform.windows = true;
    setPref("mcpEnabled", true);
    const exe = String.raw`C:\Users\O'Neil\tabler.exe`;
    const quoted = String.raw`'C:\Users\O''Neil\tabler.exe'`;
    const commandLine = `& ${quoted} --mcp`;
    const registration = `codex mcp add tabler -- ${quoted} --mcp`;
    const loadInfo = ipc.mcp.info;
    vi.spyOn(ipc.mcp, "info").mockImplementation(async () => ({
      ...(await loadInfo()),
      executable: exe,
      commandLine,
    }));
    vi.spyOn(ipc.mcp, "installCli").mockResolvedValue({
      ok: false,
      cliFound: false,
      command: registration,
      message: "Codex was not found.",
    });
    renderSettings();
    expect(await screen.findByText(commandLine)).toBeInTheDocument();
    expect(screen.getByText("PowerShell command")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Copy/ }));
    await waitFor(() => expect(clipboard.writeClipboard).toHaveBeenCalledWith(commandLine));
    fireEvent.click(installButton(/Codex/));
    await waitFor(() => expect(clipboard.writeClipboard).toHaveBeenCalledWith(registration));
    expect(toast.message).toHaveBeenCalledWith(
      "Codex's command line tool was not found",
      expect.objectContaining({ description: expect.stringContaining("paste it in PowerShell") }),
    );
  });

  it("shows the mcp.json change before writing it, keeping other servers", async () => {
    setPref("mcpEnabled", true);
    const write = vi.spyOn(ipc.mcp, "writeConfig");
    renderSettings();
    await commandShown();
    fireEvent.click(installButton(/Cursor/));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Add Tabler to Cursor?")).toBeInTheDocument();
    expect(within(dialog).getByText(/Other servers and settings/)).toBeInTheDocument();
    const added = [...dialog.querySelectorAll('[data-kind="add"]')].map((l) => l.textContent);
    expect(added.some((t) => t?.includes('"tabler"'))).toBe(true);
    expect([...dialog.querySelectorAll('[data-kind="del"]')]).toHaveLength(1);
    expect(write).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Changes" }));
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith("Added Tabler to Cursor", expect.anything()),
    );
    expect(write).toHaveBeenCalledWith("cursor", expect.stringContaining('"github"'));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());

    // Already there: nothing to confirm.
    fireEvent.click(installButton(/Cursor/));
    await waitFor(() =>
      expect(toast.success).toHaveBeenCalledWith(
        "Tabler is already set up in Cursor",
        expect.anything(),
      ),
    );
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("creates the VS Code mcp.json, and Cancel writes nothing", async () => {
    setPref("mcpEnabled", true);
    const write = vi.spyOn(ipc.mcp, "writeConfig");
    renderSettings();
    await commandShown();
    fireEvent.click(installButton(/^VS Code$/));
    let dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Tabler creates/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(write).not.toHaveBeenCalled();

    fireEvent.click(installButton(/^VS Code$/));
    dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create File" }));
    await waitFor(() => expect(write).toHaveBeenCalledWith("vsCode", null));
  });

  it("installs in VS Code Insiders' own mcp.json", async () => {
    setPref("mcpEnabled", true);
    const write = vi.spyOn(ipc.mcp, "writeConfig");
    renderSettings();
    await commandShown();
    fireEvent.click(installButton(/VS Code Insiders/));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Code - Insiders\/User\/mcp\.json/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Create File" }));
    await waitFor(() => expect(write).toHaveBeenCalledWith("vsCodeInsiders", null));
    expect(toast.success).toHaveBeenCalledWith(
      "Added Tabler to VS Code Insiders",
      expect.anything(),
    );
  });
});
