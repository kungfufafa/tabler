import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc } from "@/lib/ipc";
import { mockFingerprint, mockKnownHosts, resetMockHostKeys } from "@/lib/ipc/mock/export";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionDialog } from "./ConnectionDialog";
import { HostKeyPromptHost } from "./HostKeyPrompt";
import {
  hostKeyDeclined,
  profileKeys,
  requestHostKeyDecision,
  useConnect,
  useHostKeyPromptStore,
} from "./useConnections";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), message: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const STAGING = SEED_PROFILES.find((p) => p.id === "p-staging-ssh")!;
const BASTION_FP = mockFingerprint("bastion.example.com");

function client() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  qc.setQueryData(profileKeys.all, SEED_PROFILES);
  return qc;
}

/** `useConnect` with (or without) the prompt host mounted next to it. */
function renderConnect({ withHost = true } = {}) {
  const qc = client();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>
      {children}
      {withHost && <HostKeyPromptHost />}
    </QueryClientProvider>
  );
  return renderHook(() => useConnect(), { wrapper }).result;
}

beforeEach(() => {
  resetMockHostKeys();
  useHostKeyPromptStore.setState({ queue: [] });
  useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
  useUiStore.setState({ view: "launcher" });
});

afterEach(() => {
  vi.restoreAllMocks();
  Object.values(toast).forEach((fn) => fn.mockReset());
});

describe("SSH host-key prompt", () => {
  it("unknown key → Trust & Connect → the connect is retried and succeeds", async () => {
    const user = userEvent.setup();
    const connect = vi.spyOn(ipc.connections, "connect");
    const accept = vi.spyOn(ipc.connections, "acceptHostKey");
    const result = renderConnect();
    act(() => result.current.mutate("p-staging-ssh"));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Unknown SSH host")).toBeInTheDocument();
    expect(within(dialog).getByText("bastion.example.com:22")).toBeInTheDocument();
    expect(within(dialog).getByText("Staging via SSH")).toBeInTheDocument();
    expect(within(dialog).getByText("ED25519 key fingerprint")).toBeInTheDocument();
    expect(within(dialog).getByTestId("host-key-fingerprint")).toHaveTextContent(BASTION_FP);
    expect(BASTION_FP).toMatch(/^SHA256:2h0Y/);

    await user.click(within(dialog).getByRole("checkbox"));
    await user.click(within(dialog).getByRole("button", { name: "Trust & Connect" }));

    await waitFor(() => expect(useConnectionsStore.getState().order).toHaveLength(1));
    expect(accept).toHaveBeenCalledWith("p-staging-ssh", BASTION_FP, true);
    expect(connect).toHaveBeenCalledTimes(2);
    expect(mockKnownHosts().get("bastion.example.com:22")).toBe(BASTION_FP);
    const conn = Object.values(useConnectionsStore.getState().byId)[0]!;
    expect(conn.info.tunnel?.localPort).toBeGreaterThan(0);
    expect(useUiStore.getState().view).toBe("workspace");
    expect(toast.error).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());

    // Trusted now: the next connect goes straight through.
    act(() => result.current.mutate("p-staging-ssh"));
    await waitFor(() => expect(useConnectionsStore.getState().order).toHaveLength(2));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(accept).toHaveBeenCalledOnce();
  });

  it("cancelling the prompt leaves the connection closed without an extra toast", async () => {
    const user = userEvent.setup();
    const accept = vi.spyOn(ipc.connections, "acceptHostKey");
    const result = renderConnect();
    act(() => result.current.mutate("p-staging-ssh"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(hostKeyDeclined(result.current.error)).toBe(true);
    expect(accept).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(useConnectionsStore.getState().order).toHaveLength(0);
  });

  it("changed key → man-in-the-middle warning with no way to trust it", async () => {
    const user = userEvent.setup();
    const accept = vi.spyOn(ipc.connections, "acceptHostKey");
    const result = renderConnect();
    act(() => result.current.mutate("p-rotated-ssh"));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("SSH host key has changed")).toBeInTheDocument();
    expect(within(dialog).getByText(/man-in-the-middle/)).toBeInTheDocument();
    expect(within(dialog).getByTestId("host-key-fingerprint")).toHaveTextContent(
      mockFingerprint("rotated.example.com"),
    );
    expect(
      within(dialog).getByText('ssh-keygen -R "[rotated.example.com]:2222"'),
    ).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /Trust/ })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("checkbox")).not.toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(result.current.error).toMatchObject({ code: "SSH_HOST_KEY_CHANGED" });
    expect(accept).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("a prompt whose host unmounts releases its connect with the error toast", async () => {
    const qc = client();
    const host = render(<HostKeyPromptHost />);
    const result = renderHook(() => useConnect(), {
      wrapper: ({ children }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>,
    }).result;
    act(() => result.current.mutate("p-staging-ssh"));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();

    // E.g. the Launcher (the prompt's only mount point) goes away when another connect succeeds.
    host.unmount();
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(hostKeyDeclined(result.current.error)).toBe(false);
    expect(toast.error).toHaveBeenCalledWith("Could not connect to Staging via SSH", {
      description: "unknown host key for bastion.example.com:22",
    });
    expect(useHostKeyPromptStore.getState().queue).toHaveLength(0);
    // Nothing stale pops up when a host mounts again.
    render(<HostKeyPromptHost />);
    await act(() => Promise.resolve());
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("a host moved to another mount point in one commit keeps the pending prompt", async () => {
    const user = userEvent.setup();
    const Moving = ({ at }: { at: "a" | "b" }) =>
      at === "a" ? (
        <div>
          <HostKeyPromptHost />
        </div>
      ) : (
        <section>
          <HostKeyPromptHost />
        </section>
      );
    const view = render(<Moving at="a" />);
    const asking = requestHostKeyDecision({
      host: "bastion.example.com",
      port: 22,
      keyType: "ssh-ed25519",
      fingerprint: BASTION_FP,
      changed: false,
      profileId: STAGING.id,
      purpose: "connect",
      label: null,
    });
    expect(asking).not.toBeNull();
    await screen.findByRole("alertdialog");
    view.rerender(<Moving at="b" />);
    await act(() => Promise.resolve());
    expect(useHostKeyPromptStore.getState().queue).toHaveLength(1);
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Trust & Connect" }));
    await expect(asking).resolves.toEqual({ persist: false });
  });

  it("without a mounted prompt the error is reported as a toast", async () => {
    const result = renderConnect({ withHost: false });
    act(() => result.current.mutate("p-staging-ssh"));
    await waitFor(() => expect(result.current.isError).toBe(true));
    expect(toast.error).toHaveBeenCalledWith("Could not connect to Staging via SSH", {
      description: "unknown host key for bastion.example.com:22",
    });
  });
});

describe("ConnectionDialog", () => {
  it("summarises the tunnel and confirms the host key from the Test button", async () => {
    const user = userEvent.setup();
    const accept = vi.spyOn(ipc.connections, "acceptHostKey");
    const test = vi.spyOn(ipc.profiles, "test");
    render(
      <QueryClientProvider client={client()}>
        <ConnectionDialog open profile={STAGING} onOpenChange={() => {}} />
        <HostKeyPromptHost />
      </QueryClientProvider>,
    );
    const form = await screen.findByRole("dialog");
    expect(within(form).getByText("deploy@bastion.example.com:22")).toBeInTheDocument();

    await user.click(within(form).getByRole("button", { name: "Test" }));
    const prompt = await screen.findByRole("alertdialog");
    await user.click(within(prompt).getByRole("button", { name: "Trust & Test" }));

    expect(await within(form).findByText(/· via SSH$/)).toBeInTheDocument();
    expect(accept).toHaveBeenCalledWith(STAGING.id, BASTION_FP, false);
    expect(test).toHaveBeenCalledTimes(2);
    // Both attempts carry the form's profile id, which keys the pending host key.
    expect(test.mock.calls.every(([input]) => input.profile.id === STAGING.id)).toBe(true);
  });

  it("shows 'Host key not trusted' when the prompt is cancelled", async () => {
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client()}>
        <ConnectionDialog open profile={STAGING} onOpenChange={() => {}} />
        <HostKeyPromptHost />
      </QueryClientProvider>,
    );
    const form = await screen.findByRole("dialog");
    await user.click(within(form).getByRole("button", { name: "Test" }));
    const prompt = await screen.findByRole("alertdialog");
    await user.click(within(prompt).getByRole("button", { name: "Cancel" }));
    expect(await within(form).findByText("Host key not trusted")).toBeInTheDocument();
    // The form itself stays open.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("shows the SSH tab hints", async () => {
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client()}>
        <ConnectionDialog open profile={STAGING} onOpenChange={() => {}} />
        <HostKeyPromptHost />
      </QueryClientProvider>,
    );
    const form = await screen.findByRole("dialog");
    // The summary line jumps to the SSH tab.
    await user.click(within(form).getByTitle("Edit SSH tunnel"));
    expect(within(form).getByRole("tab", { name: "SSH" })).toHaveAttribute("aria-selected", "true");
    expect(within(form).getByText(/keepalive is/)).toBeInTheDocument();
    expect(within(form).getByText("auto")).toBeInTheDocument();
  });
});
