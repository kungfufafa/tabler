import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockConnectionFiles } from "@/lib/ipc/mock/connections_io";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { ExportConnectionsDialog } from "./ExportConnectionsDialog";
import { ImportConnectionsDialog } from "./ImportConnectionsDialog";
import type { ExportScope } from "./ioStore";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

const SCOPE: ExportScope = {
  title: "Export “Staging”",
  profileIds: ["p-staging-ssh", "p-rotated-ssh"],
  groups: ["Staging"],
  fileName: "Staging",
};
const PATH = "/Users/me/Downloads/Staging.tablerconnections";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});
beforeEach(() => resetMockConnectionFiles());
afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(toast.success).mockReset();
});

describe("ExportConnectionsDialog", () => {
  it("needs a file password to include secrets, then saves where the user picks", async () => {
    const run = vi.spyOn(ipc.profiles, "exportConnections");
    const prompt = vi.spyOn(window, "prompt").mockReturnValue(PATH);
    const onClose = vi.fn();
    const user = userEvent.setup();
    wrap(<ExportConnectionsDialog scope={SCOPE} onClose={onClose} />);
    const dialog = await screen.findByRole("dialog", { name: "Export “Staging”" });
    expect(dialog).toHaveTextContent("2 connections");
    expect(within(dialog).getByLabelText("File password (optional)")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("checkbox", { name: "Include database passwords" }));
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Set a file password");
    expect(prompt).not.toHaveBeenCalled();

    await user.click(
      within(dialog).getByRole("checkbox", { name: "Include SSH passwords and passphrases" }),
    );
    await user.type(within(dialog).getByLabelText("File password"), "pw");
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    await vi.waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(prompt).toHaveBeenCalledWith(
      "Export connections to",
      "/Users/me/Downloads/Staging.tablerconnections",
    );
    expect(run).toHaveBeenCalledWith({
      profileIds: SCOPE.profileIds,
      groups: ["Staging"],
      includePasswords: true,
      includeSshSecrets: true,
      filePassword: "pw",
      path: PATH,
    });
    expect(toast.success).toHaveBeenCalledWith("Exported 2 connections", { description: PATH });
  });

  it("does nothing when the save panel is cancelled", async () => {
    const run = vi.spyOn(ipc.profiles, "exportConnections");
    vi.spyOn(window, "prompt").mockReturnValue(null);
    const onClose = vi.fn();
    const user = userEvent.setup();
    wrap(<ExportConnectionsDialog scope={SCOPE} onClose={onClose} />);
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Export…" }));
    expect(run).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("round-trips through the import dialog: password, preview and duplicate names", async () => {
    vi.spyOn(window, "prompt").mockReturnValue(PATH);
    const user = userEvent.setup();
    const { unmount } = wrap(<ExportConnectionsDialog scope={SCOPE} onClose={() => {}} />);
    const exporting = await screen.findByRole("dialog");
    await user.type(within(exporting).getByLabelText("File password (optional)"), "secret");
    await user.click(within(exporting).getByRole("button", { name: "Export…" }));
    await vi.waitFor(() => expect(toast.success).toHaveBeenCalled());
    unmount();

    const importConnections = vi.spyOn(ipc.profiles, "importConnections");
    const onImported = vi.fn();
    const onClose = vi.fn();
    wrap(<ImportConnectionsDialog path={PATH} onClose={onClose} onImported={onImported} />);
    const dialog = await screen.findByRole("dialog", { name: "Import Connections" });
    expect(dialog).toHaveTextContent("Staging.tablerconnections");
    const password = await within(dialog).findByLabelText("File password");
    await user.type(password, "wrong{Enter}");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("password is wrong");
    await user.clear(password);
    await user.type(password, "secret{Enter}");

    const items = await within(dialog).findByRole("list", { name: "Connections in the file" });
    expect(within(items).getAllByRole("listitem")).toHaveLength(2);
    // Both names exist already (they were exported from this list).
    expect(within(items).getAllByText(/already exists/)).toHaveLength(2);
    expect(within(dialog).getByText("2 connections selected")).toBeInTheDocument();
    await user.click(within(items).getByRole("checkbox", { name: "Import Rotated host" }));
    expect(within(dialog).getByText("1 connection selected")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    await vi.waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(importConnections).toHaveBeenCalledWith(PATH, "secret", ["p-staging-ssh"]);
    expect(onImported).toHaveBeenCalledWith([
      expect.objectContaining({ name: "Staging via SSH", group: "Staging" }),
    ]);
    expect(onImported.mock.calls[0]![0][0].id).not.toBe("p-staging-ssh");
  });

  it("shows each connection's safe-mode level, Read-only and startup SQL before importing", async () => {
    const pg = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;
    const options = { ...pg.options, safeMode: null, readOnly: false, startupSql: null };
    const sneaky = {
      ...pg,
      id: "f-sneaky",
      name: "Sneaky",
      options: { ...options, safeMode: "alert" as const, startupSql: "DROP TABLE users;" },
    };
    const plain = { ...pg, id: "f-plain", name: "Plain", options: { ...options, readOnly: true } };
    vi.spyOn(ipc.profiles, "inspectConnectionsFile").mockResolvedValue({
      encrypted: false,
      needsPassword: false,
      groups: [],
      connections: [sneaky, plain].map((profile) => ({
        profile,
        duplicateName: false,
        hasPassword: false,
        hasSshSecrets: false,
      })),
    });
    const importConnections = vi.spyOn(ipc.profiles, "importConnections").mockResolvedValue([]);
    const user = userEvent.setup();
    wrap(<ImportConnectionsDialog path={PATH} onClose={() => {}} />);
    const items = await screen.findByRole("list", { name: "Connections in the file" });
    const [first, second] = within(items).getAllByRole("listitem");
    expect(first).toHaveTextContent("Alert Mode 1");
    expect(first).toHaveTextContent("Runs startup SQL on connect");
    expect(within(first!).getByLabelText("Startup SQL of Sneaky")).toHaveTextContent(
      "DROP TABLE users;",
    );
    expect(second).toHaveTextContent("Silent Mode");
    expect(second).toHaveTextContent("Read-only");
    expect(second).not.toHaveTextContent("startup SQL");
    // Only ticked connections come in.
    await user.click(within(items).getByRole("checkbox", { name: "Import Sneaky" }));
    await user.click(screen.getByRole("button", { name: "Import" }));
    await vi.waitFor(() => expect(importConnections).toHaveBeenCalledWith(PATH, null, ["f-plain"]));
  });

  it("explains a file that cannot be read", async () => {
    wrap(<ImportConnectionsDialog path="/nope.tablerconnections" onClose={() => {}} />);
    const dialog = await screen.findByRole("dialog", { name: "Import Connections" });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/No such file/);
    expect(within(dialog).getByRole("button", { name: "Import" })).toBeDisabled();
  });
});
