import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { EditorView } from "@codemirror/view";
import { act } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { usePaletteStore } from "@/features/palette/store";
import { SafetyHost } from "@/features/safety/SafetyHost";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { resetPrefsForTests, setPref } from "@/stores/prefs";
import { useUiStore } from "@/stores/ui";
import { ConnectionDialog } from "./ConnectionDialog";
import { PasswordPromptHost } from "./PasswordPrompt";
import { usePasswordPromptStore } from "./passwordPromptStore";
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
      <PasswordPromptHost />
    </QueryClientProvider>,
  );
  return { onOpenChange, onSaved };
}

beforeAll(async () => {
  // jsdom lacks these; Radix Select needs them to open and scroll to the selected item.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  // Load the mock backend before the first call (the transport imports it lazily).
  await import("@/lib/ipc/mock");
});

/** Picks `option` in the Radix select labelled `label`. */
async function choose(
  user: ReturnType<typeof userEvent.setup>,
  scope: HTMLElement,
  label: string,
  option: string,
) {
  within(scope).getByRole("combobox", { name: label }).focus();
  await user.keyboard("{Enter}");
  await user.click(await screen.findByRole("option", { name: option }));
}

beforeEach(() => {
  resetPrefsForTests();
  usePasswordPromptStore.setState({ queue: [] });
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
    expect(within(form).getByLabelText("SSL mode")).toHaveTextContent("require");
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
    expect(within(ssh).getByLabelText("Private key")).toHaveAttribute(
      "placeholder",
      "Leave empty to use ~/.ssh/config",
    );
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(within(ssh).getByText("SSH host is required")).toBeInTheDocument();
    // User and key may come from ~/.ssh/config.
    expect(within(ssh).queryByText(/SSH user is required/)).toBeNull();
    expect(save).not.toHaveBeenCalled();
    await user.type(within(ssh).getByLabelText("SSH host"), "bastion");
    await user.type(within(ssh).getByLabelText("Local port"), "15432");
    await user.type(within(ssh).getByLabelText("Keep-alive interval (s)"), "20");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile.ssh).toMatchObject({
      host: "bastion",
      user: "",
      localPort: 15432,
      keepaliveSecs: 20,
    });
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

  it("a Safe Mode connection asks for its password before Read-only is turned off", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    render(<SafetyHost />);
    const pg = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;
    renderDialog({
      profile: { ...pg, options: { ...pg.options, safeMode: "safe", readOnly: true } },
    });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    await user.click(within(form).getByRole("button", { name: "Advanced" }));
    await user.click(within(form).getByLabelText("Read-only"));
    await user.click(within(form).getByRole("button", { name: "Save" }));
    let prompt = await screen.findByRole("alertdialog", {
      name: "Enter the password of Local Postgres to continue",
    });
    expect(prompt).toHaveTextContent("Turning off Read-only.");
    fireEvent.keyDown(prompt, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(save).not.toHaveBeenCalled();

    await user.click(within(form).getByRole("button", { name: "Save" }));
    prompt = await screen.findByRole("alertdialog");
    // The mock backend stores "secret" for this profile.
    await user.type(within(prompt).getByLabelText("Password"), "secret{Enter}");
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile.options.readOnly).toBe(false);
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

describe("ConnectionDialog — TablePlus parity", () => {
  const PG = SEED_PROFILES[0]!;

  it("Test saves the connection first and keeps the form open", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const test = vi.spyOn(ipc.profiles, "test");
    const user = userEvent.setup();
    const { onOpenChange, onSaved } = renderDialog({
      prefill: parseConnectionUrl("mysql://root@127.0.0.1/shop?name=Shop"),
    });
    const form = await screen.findByRole("dialog", { name: "New MySQL / MariaDB connection" });
    await user.click(within(form).getByRole("button", { name: "Test" }));
    expect(await within(form).findByRole("status")).toHaveTextContent(/MySQL 9\.6\.0/);
    expect(save).toHaveBeenCalledTimes(1);
    expect(test).toHaveBeenCalledTimes(1);
    expect(save.mock.invocationCallOrder[0]!).toBeLessThan(test.mock.invocationCallOrder[0]!);
    const saved = await save.mock.results[0]!.value;
    expect(test.mock.calls[0]![0].profile.id).toBe(saved.id);
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ name: "Shop" }));
    expect(onOpenChange).not.toHaveBeenCalled();
    // A later Save updates the same connection.
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(toast.success).toHaveBeenCalledWith("Connection updated"));
    expect(save.mock.calls[1]![0].profile.id).toBe(saved.id);
  });

  it("Ask every time: no password is stored; Test asks for it and Connect reuses it", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const test = vi.spyOn(ipc.profiles, "test");
    const connect = vi.spyOn(ipc.connections, "connect");
    const user = userEvent.setup();
    renderDialog({ profile: { ...PG, id: "p-ask-pg", name: "Ask PG" } });
    const form = await screen.findByRole("dialog", { name: "Edit Ask PG" });
    await user.type(within(form).getByLabelText("Password"), "typed");
    await choose(user, form, "Password storage", "Ask every time");
    expect(within(form).getByLabelText("Password")).toBeDisabled();
    expect(within(form).getByLabelText("Password")).toHaveValue("");
    expect(within(form).getByText(/Asked on every connect/)).toBeInTheDocument();

    await user.click(within(form).getByRole("button", { name: "Test" }));
    const prompt = await screen.findByRole("dialog", { name: "Password for Ask PG" });
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0]![0]).toMatchObject({
      password: null,
      profile: { options: { passwordMode: "ask" } },
    });
    await user.type(within(prompt).getByLabelText("Password"), "s3cret{Enter}");
    expect(await within(form).findByRole("status")).toHaveTextContent(/PostgreSQL/);
    expect(test.mock.calls[0]![0].password).toBe("s3cret");

    // Connect does not ask again in this dialog session.
    await user.click(within(form).getByRole("button", { name: "Connect" }));
    await vi.waitFor(() => expect(connect).toHaveBeenCalledWith("p-ask-pg", "s3cret"));
    expect(screen.queryByRole("dialog", { name: "Password for Ask PG" })).toBeNull();
  });

  it("cancelling the password prompt of Test shows nothing", async () => {
    const test = vi.spyOn(ipc.profiles, "test");
    const user = userEvent.setup();
    renderDialog({
      profile: { ...PG, id: "p-ask-2", options: { ...PG.options, passwordMode: "ask" } },
    });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    expect(within(form).getByLabelText("Password storage")).toHaveTextContent("Ask every time");
    await user.click(within(form).getByRole("button", { name: "Test" }));
    const prompt = await screen.findByRole("dialog", { name: "Password for Local Postgres" });
    await user.click(within(prompt).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() =>
      expect(within(form).getByRole("button", { name: "Test" })).toBeEnabled(),
    );
    expect(test).not.toHaveBeenCalled();
    expect(within(form).queryByRole("status")).toBeNull();
  });

  it("SSL keys: Key… / Cert… / CA Cert… pick files, — clears them, a key needs its cert", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const prompt = vi.spyOn(window, "prompt");
    const user = userEvent.setup();
    renderDialog({ profile: PG });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    const keys = within(form).getByRole("group", { name: "SSL keys" });
    expect(
      within(keys)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Key…", "Cert…", "CA Cert…"]);

    prompt.mockReturnValueOnce("/certs/client-key.pem");
    await user.click(within(keys).getByRole("button", { name: "Key…" }));
    expect(within(keys).getByRole("button", { name: "Key… client-key.pem" })).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(
      await within(form).findByText("Choose the certificate that goes with the key (Cert…)"),
    ).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();

    prompt.mockReturnValueOnce("/certs/client-cert.pem");
    await user.click(within(keys).getByRole("button", { name: "Cert…" }));
    prompt.mockReturnValueOnce("/certs/ca.pem");
    await user.click(within(keys).getByRole("button", { name: "CA Cert…" }));
    await user.click(within(keys).getByRole("button", { name: "Clear CA Cert" }));
    expect(within(keys).getByRole("button", { name: "CA Cert…" })).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile).toMatchObject({
      sslCaPath: null,
      options: { sslKeyPath: "/certs/client-key.pem", sslCertPath: "/certs/client-cert.pem" },
    });
  });

  it("lists MySQL's SSL mode names for MySQL and libpq's for PostgreSQL", async () => {
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES.find((p) => p.id === "p-shop-mysql")! });
    const form = await screen.findByRole("dialog", { name: "Edit Shop MySQL" });
    expect(within(form).getByLabelText("SSL mode")).toHaveTextContent("PREFERRED");
    within(form).getByLabelText("SSL mode").focus();
    await user.keyboard("{Enter}");
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual([
      "PREFERRED",
      "DISABLED",
      "REQUIRED",
      "VERIFY-CA",
      "VERIFY-IDENTITY",
    ]);
    await user.click(screen.getByRole("option", { name: "VERIFY-IDENTITY" }));
    // Same stored value as PostgreSQL's verify-full.
    await choose(user, form, "Driver", "PostgreSQL");
    expect(within(form).getByLabelText("SSL mode")).toHaveTextContent("verify-full");
  });

  it("Bootstrap commands… edits the SQL run after login in a modal editor", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ profile: PG });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    await user.click(within(form).getByRole("button", { name: /Bootstrap commands/ }));
    const editor = await screen.findByRole("dialog", { name: "Bootstrap commands" });
    const view = EditorView.findFromDOM(
      within(editor).getByTestId("bootstrap-editor").querySelector(".cm-editor") as HTMLElement,
    )!;
    act(() =>
      view.dispatch({ changes: { from: 0, insert: "SET search_path = app;\nSET jit = off;" } }),
    );
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    expect(screen.queryByRole("dialog", { name: "Bootstrap commands" })).toBeNull();
    expect(within(form).getByTestId("bootstrap-summary")).toHaveTextContent(
      "SET search_path = app; (+1 more)",
    );
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile.options.startupSql).toBe(
      "SET search_path = app;\nSET jit = off;",
    );
  });

  it("Advanced has TablePlus's safe-mode levels; new connections start from the preference", async () => {
    setPref("defaultSafeMode", "alertNonSelect");
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ prefill: parseConnectionUrl("postgres://app@db/x?name=Safe") });
    const form = await screen.findByRole("dialog", { name: "New PostgreSQL connection" });
    await user.click(within(form).getByRole("button", { name: "Advanced" }));
    const select = within(form).getByRole("combobox", { name: "Safe mode" });
    expect(select).toHaveTextContent("Alert Mode 2");
    select.focus();
    await user.keyboard("{Enter}");
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual([
      "Silent Mode",
      "Alert Mode 1",
      "Alert Mode 2",
      "Safe Mode 1",
      "Safe Mode 2",
    ]);
    await user.click(screen.getByRole("option", { name: "Safe Mode 1" }));
    expect(within(form).getByText(/Ask for the password before sending/)).toBeInTheDocument();
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile.options.safeMode).toBe("safe");
  });
});

describe("ConnectionDialog — IAM command and MySQL socket", () => {
  const PG = SEED_PROFILES[0]!;
  const SHOP = SEED_PROFILES.find((p) => p.id === "p-shop-mysql")!;
  const IAM = "aws rds generate-db-auth-token --hostname db --port 5432 --username app";

  it("Command Line (IAM authentication) asks for the command instead of a password", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const test = vi.spyOn(ipc.profiles, "test");
    const user = userEvent.setup();
    renderDialog({ profile: PG });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    expect(within(form).queryByLabelText("Command")).toBeNull();
    await choose(user, form, "Password storage", "Command Line (IAM authentication)");
    expect(within(form).getByLabelText("Password")).toBeDisabled();
    expect(within(form).getByLabelText("Password")).toHaveAttribute(
      "placeholder",
      "From the command",
    );
    const command = within(form).getByLabelText("Command");
    expect(command).toHaveAttribute(
      "placeholder",
      expect.stringContaining("aws rds generate-db-auth-token"),
    );
    await user.click(within(form).getByRole("button", { name: "Test" }));
    expect(
      await within(form).findByText("Enter the command that prints the password"),
    ).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();

    await user.type(command, IAM);
    await user.click(within(form).getByRole("button", { name: "Test" }));
    expect(await within(form).findByRole("status")).toHaveTextContent(/PostgreSQL/);
    const input = save.mock.calls[0]![0];
    expect(input.profile.options).toMatchObject({ passwordMode: "command", passwordCommand: IAM });
    expect(input.password).toBeNull();
    // The test needs no typed password: the backend runs the command.
    expect(test.mock.calls[0]![0].password).toBeNull();
  });

  it("MySQL Use socket swaps Host / Port for the socket path and cannot go over SSH", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ profile: SHOP });
    const form = await screen.findByRole("dialog", { name: "Edit Shop MySQL" });
    expect(within(form).getByLabelText("Host")).toBeInTheDocument();
    await user.click(within(form).getByRole("checkbox", { name: "Use socket" }));
    expect(within(form).queryByLabelText("Host")).toBeNull();
    expect(within(form).queryByLabelText("Port")).toBeNull();
    const socket = within(form).getByLabelText("Socket");
    expect(socket).toHaveAttribute("placeholder", "/tmp/mysql.sock");

    await user.click(within(form).getByRole("button", { name: "Over SSH" }));
    await user.type(within(form).getByLabelText("SSH host"), "bastion");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByText(/cannot go over SSH yet/)).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();

    await user.click(within(form).getByRole("button", { name: "Over SSH" }));
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile).toMatchObject({
      ssh: null,
      options: { useSocket: true, socketPath: null },
    });
  });

  it("only MySQL offers Use socket", async () => {
    renderDialog({ profile: PG });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    expect(within(form).queryByRole("checkbox", { name: "Use socket" })).toBeNull();
  });
});

describe("ConnectionDialog — groups", () => {
  it("a prefill without a driver (New ▸ Connection in this group…) starts at the driver picker", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ prefill: { values: { group: "Clients / Acme" }, password: null } });
    const picker = await screen.findByRole("dialog", { name: "New connection" });
    await user.click(within(picker).getByRole("radio", { name: /MySQL/ }));
    await user.click(within(picker).getByRole("button", { name: "Create" }));
    const form = await screen.findByRole("dialog", { name: "New MySQL / MariaDB connection" });
    expect(within(form).getByLabelText("Group")).toHaveValue("Clients / Acme");
    expect(within(form).getByLabelText("Port")).toHaveValue("3306");
    await user.type(within(form).getByLabelText("Name"), "Acme");
    await user.type(within(form).getByLabelText("User"), "root");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile).toMatchObject({
      driver: "mysql",
      group: "Clients / Acme",
    });
  });

  it("the Group field suggests existing group paths and still takes free text", async () => {
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog({ profile: SEED_PROFILES[0]! });
    const form = await screen.findByRole("dialog", { name: "Edit Local Postgres" });
    const group = within(form).getByRole("combobox", { name: "Group" });
    await user.clear(group);
    const list = await screen.findByRole("listbox", { name: "Groups" });
    const all = within(list)
      .getAllByRole("option")
      .map((o) => o.getAttribute("title"));
    expect(all).toEqual(expect.arrayContaining(["Local", "Staging"]));

    // Typing filters; ↓ ↩ picks (without submitting the form).
    await user.type(group, "stag");
    const filtered = within(await screen.findByRole("listbox", { name: "Groups" }))
      .getAllByRole("option")
      .map((o) => o.getAttribute("title"));
    expect(filtered).toEqual(["Staging"]);
    await user.keyboard("{ArrowDown}{Enter}");
    expect(group).toHaveValue("Staging");
    expect(screen.queryByRole("listbox", { name: "Groups" })).toBeNull();
    expect(save).not.toHaveBeenCalled();

    // Esc closes the list, not the dialog.
    await user.clear(group);
    expect(await screen.findByRole("listbox", { name: "Groups" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Groups" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Edit Local Postgres" })).toBeInTheDocument();

    // A new nested path, normalised on save; longer than 200 characters is refused.
    await user.type(group, `${"x".repeat(201)}`);
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(
      await within(form).findByText("Use at most 200 characters for the group path"),
    ).toBeInTheDocument();
    await user.clear(group);
    await user.type(group, "Clients/ Acme /Prod");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0]![0].profile.group).toBe("Clients / Acme / Prod");
  });
});
