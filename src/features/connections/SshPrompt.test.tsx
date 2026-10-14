import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, toIpcError } from "@/lib/ipc";
import { mockFingerprint, resetMockHostKeys } from "@/lib/ipc/mock/export";
import { SshPromptHost } from "./SshPrompt";
import { promptLabel, useSshPromptStore } from "./sshPromptStore";

const STAGING = "p-staging-ssh";

/** Trusts the staging bastion's host key, so connects get to the 2FA question. */
async function trustBastion() {
  await ipc.connections.connect(STAGING).catch(() => undefined);
  await ipc.connections.acceptHostKey(STAGING, mockFingerprint("bastion.example.com"));
}

const codeOf = (err: unknown) => toIpcError(err).code;

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});

beforeEach(async () => {
  resetMockHostKeys();
  useSshPromptStore.setState({ queue: [] });
  await trustBastion();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("SSH keyboard-interactive prompt", () => {
  it("asks the server's question; a wrong code fails, six digits connect", async () => {
    const answer = vi.spyOn(ipc.connections, "answerSshPrompt");
    const user = userEvent.setup();
    render(<SshPromptHost />);

    const wrong = ipc.connections.connect(STAGING).catch(codeOf);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("SSH authentication")).toBeInTheDocument();
    expect(within(dialog).getByText("deploy@bastion.example.com")).toBeInTheDocument();
    expect(within(dialog).getByText("Staging via SSH")).toBeInTheDocument();
    expect(within(dialog).getByTestId("ssh-prompt-instructions")).toHaveTextContent(
      /Two-factor authentication is required/,
    );
    const code = within(dialog).getByLabelText("Verification code");
    expect(code).toHaveAttribute("type", "password");
    await waitFor(() => expect(code).toHaveFocus());
    await user.type(code, "12345{Enter}");
    expect(await wrong).toBe("AUTH_FAILED");
    expect(answer).toHaveBeenCalledWith(expect.any(String), ["12345"]);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());

    const right = ipc.connections.connect(STAGING);
    const again = await screen.findByRole("alertdialog");
    await user.type(within(again).getByLabelText("Verification code"), "654321");
    await user.click(within(again).getByRole("button", { name: "Connect" }));
    await expect(right).resolves.toMatchObject({ profileId: STAGING });
    expect(useSshPromptStore.getState().queue).toEqual([]);
  });

  it("Cancel gives the connect up; Test says Test", async () => {
    const user = userEvent.setup();
    render(<SshPromptHost />);
    const profile = (await ipc.profiles.list()).find((p) => p.id === STAGING)!;
    const tested = ipc.profiles
      .test({
        profile,
        password: null,
        sshPassword: null,
        sshPassphrase: null,
        clearPassword: false,
      })
      .catch(codeOf);
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("button", { name: "Test" })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(await tested).toBe("CANCELLED");
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("without a prompt host nobody is asked", async () => {
    await expect(ipc.connections.connect(STAGING)).resolves.toMatchObject({
      profileId: STAGING,
    });
  });

  it("labels drop the server's trailing colon", () => {
    expect(promptLabel("Verification code: ")).toBe("Verification code");
    expect(promptLabel("Password:")).toBe("Password");
    expect(promptLabel("  ")).toBe("Answer");
  });
});
