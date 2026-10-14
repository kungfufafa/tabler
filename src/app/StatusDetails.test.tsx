import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { StatusDetails } from "./StatusDetails";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const INFO: ConnectionInfo = {
  connectionId: "c1",
  profileId: "p1",
  name: "Dev",
  color: "blue",
  driver: "postgres",
  serverVersion: "PostgreSQL 17.6",
  currentDatabase: "app",
  currentUser: "me",
  tunnel: null,
  readOnly: false,
};

/** A stored profile (only its colour matters here). */
let stored: ConnectionProfile;

/** What `ipc.profiles.save` was given, in order, and how many saves ran at the same time. */
const saves: string[] = [];
let running = 0;
let overlap = 0;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The round trips (`get`, `save`) still waiting for the test to let them finish. */
const waiting: Array<() => void> = [];
const roundTrip = () => new Promise<void>((resolve) => waiting.push(resolve));

/**
 * `get` and `save` do not answer until `settle()` lets them: as slow as the test wants, and not
 * dependent on the speed of the machine it runs on.
 */
function slowProfiles() {
  vi.spyOn(ipc.profiles, "get").mockImplementation(async () => {
    await roundTrip();
    return { ...stored };
  });
  vi.spyOn(ipc.profiles, "save").mockImplementation(async (req) => {
    running++;
    overlap = Math.max(overlap, running);
    await roundTrip();
    stored = { ...req.profile };
    saves.push(req.profile.color ?? "");
    running--;
    return { ...req.profile };
  });
}

/** Lets the round trips finish, and the ones they lead to, until nothing is left waiting. */
async function settle() {
  for (let i = 0; i < 50; i++) {
    await act(() => sleep(0));
    if (waiting.length === 0) return;
    waiting.splice(0).forEach((finish) => finish());
  }
  throw new Error("the colour row never settled");
}

/** The status details of connection `c1`, following the store as the app does. */
function Harness() {
  const conn = useConnectionsStore((s) => s.byId["c1"]);
  if (!conn) return null;
  return <StatusDetails conn={conn} profile={null} details={null} onClose={() => {}} />;
}

function mount() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <Harness />
    </QueryClientProvider>,
  );
}

const storeColor = () => useConnectionsStore.getState().byId["c1"]?.info.color;

beforeEach(() => {
  saves.length = 0;
  waiting.length = 0;
  running = 0;
  overlap = 0;
  stored = { color: "blue" } as ConnectionProfile;
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useConnectionsStore.getState().add(INFO);
});
afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

async function openPicker(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("radio", { name: "Custom…" }));
  return (await screen.findByLabelText("Colour picker")) as HTMLInputElement;
}

describe("the colour row of the status details", () => {
  it("saves the colour a drag in the custom picker was released on", async () => {
    slowProfiles();
    const user = userEvent.setup();
    mount();
    const picker = await openPicker(user);
    // Three steps of one drag while the save of the first is still on its way.
    fireEvent.change(picker, { target: { value: "#101010" } });
    fireEvent.change(picker, { target: { value: "#202020" } });
    fireEvent.change(picker, { target: { value: "#303030" } });
    await settle();
    // The first step is saved, then the one the drag ended on; the one in between is skipped.
    expect(saves).toEqual(["#101010", "#303030"]);
    expect(stored.color).toBe("#303030");
    expect(storeColor()).toBe("#303030");
    // One save at a time.
    expect(overlap).toBe(1);
    // The picker did not snap back to an earlier colour.
    expect((await screen.findByLabelText("Colour picker")) as HTMLInputElement).toHaveValue(
      "#303030",
    );
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("does not save a second time while the first is running", async () => {
    slowProfiles();
    const user = userEvent.setup();
    mount();
    const picker = await openPicker(user);
    fireEvent.change(picker, { target: { value: "#101010" } });
    // Held back: the first save has not even read the profile yet.
    await act(() => sleep(0));
    expect(saves).toEqual([]);
    fireEvent.change(picker, { target: { value: "#202020" } });
    await act(() => sleep(0));
    expect(running).toBeLessThanOrEqual(1);
    expect(ipc.profiles.get).toHaveBeenCalledTimes(1);
    await settle();
    expect(saves).toEqual(["#101010", "#202020"]);
    expect(overlap).toBe(1);
  });

  it("does not save again when the drag came back to the colour being saved", async () => {
    slowProfiles();
    const user = userEvent.setup();
    mount();
    const picker = await openPicker(user);
    fireEvent.change(picker, { target: { value: "#101010" } });
    fireEvent.change(picker, { target: { value: "#202020" } });
    fireEvent.change(picker, { target: { value: "#101010" } });
    await settle();
    expect(saves).toEqual(["#101010"]);
    expect(storeColor()).toBe("#101010");
  });

  it("keeps working for the next drag", async () => {
    slowProfiles();
    const user = userEvent.setup();
    mount();
    const picker = await openPicker(user);
    fireEvent.change(picker, { target: { value: "#111111" } });
    await settle();
    fireEvent.change(picker, { target: { value: "#222222" } });
    fireEvent.change(picker, { target: { value: "#333333" } });
    await settle();
    expect(saves).toEqual(["#111111", "#222222", "#333333"]);
    expect(overlap).toBe(1);
  });

  it("saves a single click on a preset once, as before", async () => {
    slowProfiles();
    const user = userEvent.setup();
    mount();
    await user.click(screen.getByRole("radio", { name: "red" }));
    await settle();
    expect(saves).toEqual(["red"]);
    expect(storeColor()).toBe("red");
    expect(screen.getByRole("radio", { name: "red" })).toHaveAttribute("aria-checked", "true");
    // The colour already shown is not saved again.
    await user.click(screen.getByRole("radio", { name: "red" }));
    await settle();
    expect(saves).toEqual(["red"]);
  });

  it("reports a failed save and lets the next pick through", async () => {
    slowProfiles();
    vi.mocked(ipc.profiles.save).mockRejectedValueOnce(new Error("disk full"));
    const user = userEvent.setup();
    mount();
    const picker = await openPicker(user);
    fireEvent.change(picker, { target: { value: "#101010" } });
    fireEvent.change(picker, { target: { value: "#202020" } });
    await settle();
    expect(toast.error).toHaveBeenCalledWith(
      "Could not change the colour",
      expect.objectContaining({ description: expect.stringContaining("disk full") }),
    );
    expect(storeColor()).toBe("blue");
    fireEvent.change(picker, { target: { value: "#303030" } });
    await settle();
    expect(saves).toEqual(["#303030"]);
    expect(storeColor()).toBe("#303030");
  });
});
