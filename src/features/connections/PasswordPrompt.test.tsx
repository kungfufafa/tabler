import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { toast } from "sonner";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { mockProfiles } from "@/lib/ipc/mock/handlers";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { PasswordPromptHost } from "./PasswordPrompt";
import { useOpenProfile } from "./openProfile";
import {
  askConnectionPassword,
  handOverPassword,
  isPromptCancelled,
  requestPassword,
  takeHandedOverPassword,
  usePasswordPromptStore,
} from "./passwordPromptStore";
import { profileKeys, useConnect } from "./useConnections";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), message: vi.fn() }),
}));

const ASK: ConnectionProfile = {
  ...SEED_PROFILES[0]!,
  id: "p-ask",
  name: "Ask me",
  options: { ...SEED_PROFILES[0]!.options, passwordMode: "ask" },
};

function renderConnect({ withHost = true } = {}) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  qc.setQueryData(profileKeys.all, [...SEED_PROFILES, ASK]);
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>
      {children}
      {withHost && <PasswordPromptHost />}
    </QueryClientProvider>
  );
  return renderHook(() => useConnect(), { wrapper }).result;
}

beforeAll(async () => {
  await import("@/lib/ipc/mock");
  mockProfiles.set(ASK.id, ASK);
});
beforeEach(() => {
  usePasswordPromptStore.setState({ queue: [] });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useUiStore.setState({ view: "launcher" });
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.mocked(toast.error).mockReset();
});

describe("“Ask every time” connections", () => {
  it("ask for the password on connect and pass it to the backend", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const user = userEvent.setup();
    const result = renderConnect();
    act(() => result.current.mutate(ASK.id));
    const prompt = await screen.findByRole("dialog", { name: "Password for Ask me" });
    expect(prompt).toHaveTextContent(/kept in memory while the connection is open/);
    await user.type(within(prompt).getByLabelText("Password"), "pw{Enter}");
    await waitFor(() => expect(useConnectionsStore.getState().order).toHaveLength(1));
    expect(connect).toHaveBeenCalledWith(ASK.id, "pw");
    expect(useUiStore.getState().view).toBe("workspace");
  });

  it("a cancelled prompt connects nothing and shows no error", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const user = userEvent.setup();
    const result = renderConnect();
    act(() => result.current.mutate(ASK.id));
    const prompt = await screen.findByRole("dialog", { name: "Password for Ask me" });
    await user.click(within(prompt).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(isPromptCancelled(result.current.error)).toBe(true);
    expect(connect).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("a wrong password is reported like any failed connect", async () => {
    const user = userEvent.setup();
    const result = renderConnect();
    act(() => result.current.mutate(ASK.id));
    const prompt = await screen.findByRole("dialog", { name: "Password for Ask me" });
    await user.type(within(prompt).getByLabelText("Password"), "wrong{Enter}");
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(toast.error).toHaveBeenCalledWith("Could not connect to Ask me", {
      description: "authentication failed: wrong password",
    });
  });

  it("uses a password handed over by the connection form instead of asking", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const result = renderConnect();
    handOverPassword(ASK.id, "from-form");
    act(() => result.current.mutate(ASK.id));
    await waitFor(() => expect(connect).toHaveBeenCalledWith(ASK.id, "from-form"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("drops a handed-over password when the profile is already open", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const qc = new QueryClient();
    qc.setQueryData(profileKeys.all, [...SEED_PROFILES, ASK]);
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const open = renderHook(() => useOpenProfile(), { wrapper }).result;
    useConnectionsStore.getState().add({
      connectionId: "c-ask",
      profileId: ASK.id,
      name: ASK.name,
      color: null,
      driver: "postgres",
      serverVersion: "17",
      currentDatabase: "tabler_dev",
      currentUser: "tabler",
      tunnel: null,
      readOnly: false,
    });
    useConnectionsStore.getState().setActive(null);
    handOverPassword(ASK.id, "from-form");
    act(() => open.current(ASK.id));
    expect(useConnectionsStore.getState().activeId).toBe("c-ask");
    expect(connect).not.toHaveBeenCalled();
    // A later connect (after a disconnect) asks again instead of reusing it.
    expect(takeHandedOverPassword(ASK.id)).toBeUndefined();
  });

  it("Keychain profiles connect without a prompt or a password argument", async () => {
    const connect = vi.spyOn(ipc.connections, "connect");
    const result = renderConnect();
    act(() => result.current.mutate("p-local-pg"));
    await waitFor(() => expect(connect).toHaveBeenCalledWith("p-local-pg"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("calls the secret what the driver calls it", async () => {
    const user = userEvent.setup();
    renderConnect();
    const asked = askConnectionPassword("Edge", "test", "d1");
    const prompt = await screen.findByRole("dialog", { name: "API Token for Edge" });
    expect(prompt).toHaveTextContent(/asks for its API token every time/);
    await user.type(within(prompt).getByLabelText("Password"), "tok{Enter}");
    await expect(asked).resolves.toBe("tok");
    // The core drivers' prompts are as before, SQLite's key included.
    const lite = askConnectionPassword("Lite", "connect", "sqlite");
    const again = await screen.findByRole("dialog", { name: "Password for Lite" });
    expect(again).toHaveTextContent(/asks for its password every time/);
    await user.type(within(again).getByLabelText("Password"), "k{Enter}");
    await expect(lite).resolves.toBe("k");
  });

  it("without a prompt host nothing is asked: the request resolves null", async () => {
    await expect(requestPassword({ title: "x" })).resolves.toBeNull();
  });
});
