import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeLane } from "@/features/drivers/__fixtures__/fakeLane";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { registerLaneOverride } from "@/features/drivers/registry";
import type { LaneModule } from "@/features/drivers/types";
import { ipc, type ConnectionProfile, type DatabaseKind } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useAppInfoStore } from "@/stores/appInfo";
import { resetPrefsForTests } from "@/stores/prefs";
import { ConnectionDialog } from "./ConnectionDialog";
import { openConnectionDialog, useConnectionDialog } from "./store";
import { profileKeys } from "./useConnections";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

// The connection form of a lane's database (F9b), with the fake lane standing in for MongoDB's
// (a document store: host, user, secret and an "Auth source" kept in the driver params) and for
// DynamoDB's (a cloud database: no SSH).

function renderDialog(profile: ConnectionProfile | null = null) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  qc.setQueryData(profileKeys.all, SEED_PROFILES);
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={qc}>
      <ConnectionDialog open profile={profile} onOpenChange={onOpenChange} />
    </QueryClientProvider>,
  );
  return { onOpenChange };
}

const built = (...kinds: DatabaseKind[]) =>
  useAppInfoStore.setState({
    info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, ...kinds] },
  });

/** A lane whose module arrives when the test says so. */
function deferredLane(module: LaneModule) {
  let resolve: () => void = () => {};
  const loaded = new Promise<{ default: LaneModule }>((r) => {
    resolve = () => r({ default: module });
  });
  return { load: () => loaded, resolve };
}

const mongo: ConnectionProfile = {
  ...SEED_PROFILES[0]!,
  id: "p-docs",
  name: "Docs",
  driver: "mongodb",
  host: "db.example",
  port: 27018,
  user: null,
  database: null,
  options: { ...SEED_PROFILES[0]!.options, driverParams: { authSource: "audit" } },
};

beforeAll(async () => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  await import("@/lib/ipc/mock");
});

beforeEach(() => {
  resetPrefsForTests();
  registerLaneOverride("mongodb", fakeLane("mongodb", "mongodb"));
  registerLaneOverride("dynamodb", fakeLane("dynamodb", "dynamodb"));
});
afterEach(() => {
  registerLaneOverride("mongodb", null);
  registerLaneOverride("dynamodb", null);
  useAppInfoStore.setState({ info: null });
  vi.restoreAllMocks();
});

const tiles = async () =>
  within(await screen.findByRole("radiogroup", { name: "Database" }))
    .getAllByRole("radio")
    .map((t) => t.textContent);

describe("the picker", () => {
  it("shows a lane's tile only when this build has its database", async () => {
    renderDialog();
    expect(await tiles()).toEqual(["PGPostgreSQL", "MYMySQL", "MAMariaDB", "SQSQLite"]);
  });

  it("lists every database of a fuller build, in the picker's order", async () => {
    built("mongodb", "cockroachdb");
    renderDialog();
    expect(await tiles()).toEqual([
      "PGPostgreSQL",
      "CRCockroachDB",
      "MYMySQL",
      "MAMariaDB",
      "SQSQLite",
      "MOMongoDB",
    ]);
  });
});

describe("a lane's form", () => {
  it("loads with its lane: the tile opens the form once the lane's fields are in", async () => {
    built("mongodb");
    const lane = deferredLane(fakeLane("mongodb", "mongodb"));
    registerLaneOverride("mongodb", lane.load);
    const user = userEvent.setup();
    renderDialog();
    await user.dblClick(await screen.findByRole("radio", { name: "MongoDB" }));
    // Still the picker: the lane has not loaded.
    expect(screen.getByRole("dialog", { name: "New connection" })).toBeInTheDocument();
    await act(async () => lane.resolve());
    const form = await screen.findByRole("dialog", { name: "New MongoDB connection" });
    expect(within(form).getByTestId("fake-fields")).toHaveTextContent("Options of mongodb");
    // The lane's defaults and its words for the shared rows.
    expect(within(form).getByLabelText("Port")).toHaveValue("27017");
    expect(within(form).getByLabelText("Auth source")).toBeInTheDocument();
    expect(within(form).getByLabelText("Driver")).toHaveTextContent("MongoDB");
  });

  it("saves the driver params and drops the host and user fields it does not use", async () => {
    built("mongodb");
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog();
    await user.dblClick(await screen.findByRole("radio", { name: "MongoDB" }));
    const form = await screen.findByRole("dialog", { name: "New MongoDB connection" });
    await user.type(within(form).getByLabelText("Name"), "Docs");
    await user.type(within(form).getByLabelText("User"), "reader");
    await user.type(within(form).getByLabelText("Auth source"), "audit");
    await user.click(within(form).getByRole("button", { name: "Save" }));
    await vi.waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    const input = save.mock.calls[0]![0];
    expect(input.profile).toMatchObject({
      name: "Docs",
      driver: "mongodb",
      host: "127.0.0.1",
      port: 27017,
      user: null,
      database: null,
      options: { driverParams: { authSource: "audit" }, pgFlavor: null },
    });
  });

  it("runs the lane's own checks next to the schema's", async () => {
    built("mongodb");
    const save = vi.spyOn(ipc.profiles, "save");
    const user = userEvent.setup();
    renderDialog();
    await user.dblClick(await screen.findByRole("radio", { name: "MongoDB" }));
    const form = await screen.findByRole("dialog", { name: "New MongoDB connection" });
    await user.type(within(form).getByLabelText("Name"), "Docs");
    await user.clear(within(form).getByLabelText("Host"));
    await user.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByText("Host is required")).toBeInTheDocument();
    expect(save).not.toHaveBeenCalled();
  });

  it("reads an edited profile's params into the lane's fields once the lane has loaded", async () => {
    built("mongodb");
    const lane = deferredLane(fakeLane("mongodb", "mongodb"));
    registerLaneOverride("mongodb", lane.load);
    renderDialog(mongo);
    const form = await screen.findByRole("dialog", { name: "Edit Docs" });
    expect(within(form).getByRole("status")).toHaveTextContent("Loading the MongoDB form…");
    await act(async () => lane.resolve());
    expect(await within(form).findByLabelText("Auth source")).toHaveValue("audit");
    expect(within(form).getByLabelText("Host")).toHaveValue("db.example");
    expect(within(form).getByLabelText("Port")).toHaveValue("27018");
  });

  it("offers SSH as the descriptor says, and Driver ▸ the same family only", async () => {
    built("mongodb", "dynamodb", "redis");
    const user = userEvent.setup();
    renderDialog(mongo);
    const form = await screen.findByRole("dialog", { name: "Edit Docs" });
    await within(form).findByLabelText("Auth source");
    // MongoDB goes through an SSH tunnel (one node).
    expect(within(form).getByRole("button", { name: "Over SSH" })).toBeInTheDocument();
    within(form).getByLabelText("Driver").focus();
    await user.keyboard("{Enter}");
    // The document stores this build has: no core driver, no Redis.
    expect((await screen.findAllByRole("option")).map((o) => o.textContent)).toEqual([
      "MongoDB",
      "DynamoDB",
    ]);
    await user.click(screen.getByRole("option", { name: "DynamoDB" }));
    await vi.waitFor(() =>
      expect(within(form).getByLabelText("Driver")).toHaveTextContent("DynamoDB"),
    );
    // DynamoDB is a cloud service: no SSH.
    expect(within(form).queryByRole("button", { name: "Over SSH" })).toBeNull();
  });

  it("falls back to its descriptor's rows while its lane gives no form", async () => {
    built("redis");
    registerLaneOverride("redis", {});
    renderDialog({ ...mongo, id: "p-cache", name: "Cache", driver: "redis", port: 6379 });
    const form = await screen.findByRole("dialog", { name: "Edit Cache" });
    expect(await within(form).findByLabelText("DB index")).toBeInTheDocument();
    expect(within(form).getByLabelText("Port")).toHaveAttribute("placeholder", "6379");
    // Redis's TLS is on or off.
    expect(within(form).getByLabelText("SSL mode")).toBeInTheDocument();
    registerLaneOverride("redis", null);
  });

  it("starts an HTTP API on HTTPS, and an on/off switch on Off", async () => {
    built("elasticsearch", "redis");
    registerLaneOverride("elasticsearch", {});
    registerLaneOverride("redis", {});
    const user = userEvent.setup();
    const sslOf = async (label: string) => {
      renderDialog();
      await user.dblClick(await screen.findByRole("radio", { name: label }));
      const form = await screen.findByRole("dialog", { name: `New ${label} connection` });
      const ssl = within(form).getByLabelText("SSL mode").textContent;
      cleanup();
      return ssl;
    };
    // The new form's "prefer" is on neither menu.
    expect(await sslOf("Elasticsearch")).toBe("HTTPS");
    expect(await sslOf("Redis")).toBe("Off");
    registerLaneOverride("elasticsearch", null);
    registerLaneOverride("redis", null);
  });
});

describe("Edit Connection… of a lane's profile", () => {
  it("opens when this build has its database, and is refused otherwise", () => {
    useConnectionDialog.setState({ open: false, profile: null, prefill: null });
    openConnectionDialog(mongo);
    expect(useConnectionDialog.getState().open).toBe(false);
    built("mongodb");
    openConnectionDialog(mongo);
    expect(useConnectionDialog.getState()).toMatchObject({ open: true, profile: mongo });
    // A PostgreSQL flavor stays editable in the PostgreSQL form of any build, as before.
    useConnectionDialog.setState({ open: false, profile: null, prefill: null });
    useAppInfoStore.setState({ info: null });
    const crdb = {
      ...SEED_PROFILES[0]!,
      options: { ...SEED_PROFILES[0]!.options, pgFlavor: "cockroachdb" as const },
    };
    openConnectionDialog(crdb);
    expect(useConnectionDialog.getState().open).toBe(true);
  });
});
