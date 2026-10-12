import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionDialog } from "./ConnectionDialog";
import { parseConnectionUrl, type ParsedConnectionUrl } from "./url";
import { profileKeys } from "./useConnections";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

const SQLITE = SEED_PROFILES.find((p) => p.id === "p-sample-sqlite")!;
const STAGING = SEED_PROFILES.find((p) => p.id === "p-staging-ssh")!;

function renderDialog({
  profile = null,
  prefill = null,
}: { profile?: ConnectionProfile | null; prefill?: ParsedConnectionUrl | null } = {}) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  qc.setQueryData(profileKeys.all, SEED_PROFILES);
  const onOpenChange = vi.fn();
  const onSaved = vi.fn();
  render(
    <QueryClientProvider client={qc}>
      <ConnectionDialog
        open
        profile={profile}
        prefill={prefill}
        onOpenChange={onOpenChange}
        onSaved={onSaved}
      />
    </QueryClientProvider>,
  );
  return { onOpenChange, onSaved };
}

beforeEach(() => {
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  usePaletteStore.setState({ connecting: [] });
  useUiStore.setState({ view: "launcher" });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(toast.success).mockReset();
});

describe("ConnectionDialog — new connection", () => {
  it("starts with the driver picker; a double-click on a tile opens the form for it", async () => {
    const user = userEvent.setup();
    renderDialog();
    const picker = await screen.findByRole("dialog", { name: "New connection" });
    const tiles = within(picker).getAllByRole("radio");
    expect(tiles.map((t) => t.textContent)).toEqual([
      "PGPostgreSQL",
      "MYMySQL / MariaDB",
      "SQSQLite",
    ]);
    expect(tiles[0]).toHaveAttribute("aria-checked", "true");
    await user.dblClick(within(picker).getByRole("radio", { name: /MySQL/ }));
    const form = await screen.findByRole("dialog", { name: "New MySQL / MariaDB connection" });
    expect(within(form).getByLabelText("Port")).toHaveValue("3306");
    expect(within(form).getByLabelText("Name")).toHaveFocus();
  });

  it("the picker works from the keyboard: → selects the next tile, ↩ creates", async () => {
    const user = userEvent.setup();
    renderDialog();
    const picker = await screen.findByRole("dialog", { name: "New connection" });
    expect(within(picker).getByRole("radio", { name: /PostgreSQL/ })).toHaveFocus();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(within(picker).getByRole("radio", { name: /SQLite/ })).toHaveFocus();
    expect(within(picker).getByRole("radio", { name: /SQLite/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await user.keyboard("{Enter}");
    const form = await screen.findByRole("dialog", { name: "New SQLite connection" });
    expect(within(form).getByLabelText("Database file")).toBeInTheDocument();
    // No SSH for SQLite; Advanced carries the foreign-keys switch.
    expect(within(form).queryByRole("button", { name: "Over SSH" })).toBeNull();
    await user.click(within(form).getByRole("button", { name: "Advanced" }));
    expect(within(form).getByLabelText("Foreign keys")).toBeChecked();
  });

  it("Save validates, stores the profile with the default tag and closes", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    const { onOpenChange, onSaved } = renderDialog();
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", { name: "Create" }),
    );
    const form = await screen.findByRole("dialog", { name: "New PostgreSQL connection" });
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(within(form).getByText("Name is required")).toBeInTheDocument();
    expect(within(form).getByText("User is required")).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();

    await user.type(within(form).getByLabelText("Name"), "Analytics");
    await user.type(within(form).getByLabelText("User"), "reader");
    await user.type(within(form).getByLabelText("Password"), "pw");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(save).toHaveBeenCalledTimes(1);
    const input = save.mock.calls[0]![0];
    expect(input.profile).toMatchObject({
      name: "Analytics",
      driver: "postgres",
      host: "127.0.0.1",
      port: 5432,
      user: "reader",
      options: { tag: "local" },
    });
    expect(input.password).toBe("pw");
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ name: "Analytics" }));
    expect(toast.success).toHaveBeenCalledWith("Connection created");
    // Save alone does not connect.
    expect(useConnectionsStore.getState().order).toEqual([]);
  });
});

describe("ConnectionDialog — form", () => {
  it("Connect saves, then connects the saved profile (↩ in a field does the same)", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const connect = vi.spyOn(ipc.connections, "connect");
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog({
      prefill: parseConnectionUrl("mysql://root@127.0.0.1/shop?name=Shop%20copy"),
    });
    const form = await screen.findByRole("dialog", { name: "New MySQL / MariaDB connection" });
    await user.type(within(form).getByLabelText("Database"), "{Enter}");
    await vi.waitFor(() => expect(connect).toHaveBeenCalledTimes(1));
    const saved = await save.mock.results[0]!.value;
    expect(connect).toHaveBeenCalledWith(saved.id);
    expect(onOpenChange).toHaveBeenCalledWith(false);
    await vi.waitFor(() => expect(useUiStore.getState().view).toBe("workspace"));
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("Connect on an edited profile that is already open switches to it and says how to apply", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    useConnectionsStore.getState().add({
      connectionId: "c1",
      profileId: "p-local-pg",
      name: "Local Postgres",
      color: "blue",
      driver: "postgres",
      serverVersion: "17",
      currentDatabase: "tabler_dev",
      currentUser: "tabler",
      tunnel: null,
      readOnly: false,
    });
    useConnectionsStore.setState({ activeId: null });
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES[0]! });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    await user.click(within(form).getByRole("button", { name: "Connect" }));
    await vi.waitFor(() => expect(useConnectionsStore.getState().activeId).toBe("c1"));
    expect(connect).not.toHaveBeenCalled();
    expect(toast.message).toHaveBeenCalledWith("Connection updated", {
      description: expect.stringMatching(/^Reconnect \(.+\) to apply/),
    });
  });

  it("a URL prefill skips the picker and keeps the URL's password for the Keychain", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({
      prefill: parseConnectionUrl(
        "postgresql://app:s3cret@db.example.com:6543/shop?name=Shop&sslmode=require",
      ),
    });
    const form = await screen.findByRole("dialog", { name: "New PostgreSQL connection" });
    expect(within(form).getByLabelText("Name")).toHaveValue("Shop");
    expect(within(form).getByLabelText("Host")).toHaveValue("db.example.com");
    expect(within(form).getByLabelText("Port")).toHaveValue("6543");
    expect(within(form).getByLabelText("User")).toHaveValue("app");
    expect(within(form).getByLabelText("Database")).toHaveValue("shop");
    expect(within(form).getByLabelText("SSL mode")).toHaveTextContent("Require");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0]).toMatchObject({
      password: "s3cret",
      profile: { host: "db.example.com", port: 6543, database: "shop", ssl: "require" },
    });
  });

  it("edits an existing profile in place: tag, colour and driver as selects/swatches", async () => {
    renderDialog({ profile: STAGING });
    const form = await screen.findByRole("dialog", { name: "Edit Staging via SSH" });
    expect(within(form).queryByRole("radiogroup", { name: "Database driver" })).toBeNull();
    expect(within(form).getByLabelText("Tag")).toHaveTextContent("Staging");
    expect(within(form).getByLabelText("Driver")).toHaveTextContent("PostgreSQL");
    expect(within(form).getByRole("radio", { name: "purple" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(within(form).getByRole("button", { name: "Connect" })).toHaveAttribute("type", "submit");
  });

  it("Over SSH reveals the tunnel fields inline and validates them", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES[0]! });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    const toggle = within(form).getByRole("button", { name: "Over SSH" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(within(form).queryByLabelText("SSH host")).toBeNull();
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    const ssh = within(form).getByRole("region", { name: "SSH tunnel" });
    await vi.waitFor(() => expect(within(ssh).getByLabelText("SSH host")).toHaveFocus());
    expect(within(ssh).getByLabelText("Private key")).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(within(ssh).getByText("SSH host is required")).toBeInTheDocument();
    expect(within(ssh).getByText("SSH user is required")).toBeInTheDocument();
    expect(within(ssh).getByText("Private key path is required")).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
  });

  it("Advanced holds read-only, time zone and timeout, and opens itself on an invalid value", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES[0]! });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    const advanced = within(form).getByRole("button", { name: "Advanced" });
    expect(advanced).toHaveAttribute("aria-expanded", "false");
    expect(within(form).queryByLabelText("Read-only")).toBeNull();
    await user.click(advanced);
    expect(within(form).getByLabelText("Read-only")).not.toBeChecked();
    expect(within(form).getByLabelText("Time zone")).toBeInTheDocument();
    await user.type(within(form).getByLabelText("Connect timeout"), "soon");
    await user.click(advanced);
    expect(within(form).queryByLabelText("Connect timeout")).toBeNull();
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByText("Must be a number")).toBeInTheDocument();
    expect(advanced).toHaveAttribute("aria-expanded", "true");
    expect(save).not.toHaveBeenCalled();
  });

  it("SQLite profiles show the database file with Choose… and New…", async () => {
    renderDialog({ profile: SQLITE });
    const form = await screen.findByRole("dialog", { name: "Edit sample.sqlite" });
    expect(within(form).getByLabelText("Database file")).toHaveValue(SQLITE.sqlitePath);
    expect(within(form).getByRole("button", { name: /Choose/ })).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: /New…/ })).toBeInTheDocument();
    expect(within(form).queryByLabelText("Host")).toBeNull();
  });

  it("Test shows the server version and latency", async () => {
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES[0]! });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    await user.click(within(form).getByRole("button", { name: "Test" }));
    expect(await within(form).findByRole("status")).toHaveTextContent(/PostgreSQL.* · 12 ms$/);
  });
});
