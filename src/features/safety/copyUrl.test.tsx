import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render as rtlRender, screen, waitFor, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type ConnectionProfile, type SafeModeLevel } from "@/lib/ipc";
import { getPrefs, resetPrefsForTests, setPref } from "@/stores/prefs";
import { COPY_WITH_PASSWORD_TITLE, copyConnectionUrl } from "./copyUrl";
import { SafetyHost } from "./SafetyHost";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

let clipboard: string | null = null;

const render = (ui: ReactElement) =>
  rtlRender(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);

beforeEach(() => {
  localStorage.clear();
  resetPrefsForTests();
  clipboard = null;
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      writeText: vi.fn(async (text: string) => {
        clipboard = text;
      }),
    },
  });
});
afterEach(() => vi.restoreAllMocks());

/** The seeded Local Postgres (stored password "secret") at `level`. */
async function localPg(level: SafeModeLevel = "silent"): Promise<ConnectionProfile> {
  const p = await ipc.profiles.get("p-local-pg");
  const profile = { ...p, options: { ...p.options, safeMode: level } };
  vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile);
  return profile;
}

const withPassword = (url: string | null) => url?.includes(":secret@") ?? false;

describe("Copy as URL", () => {
  it("(without password) never reveals the password nor asks", async () => {
    const reveal = vi.spyOn(ipc.security, "revealPassword");
    const profile = await localPg("safe");
    render(<SafetyHost />);
    expect(await copyConnectionUrl(profile, false)).toBe(true);
    expect(reveal).not.toHaveBeenCalled();
    expect(clipboard).toMatch(/^postgresql:\/\/[^:@]+@/);
    expect(withPassword(clipboard)).toBe(false);
  });

  it("copies an SSH profile in the +ssh form with its colour and tag, never the SSH password", async () => {
    setPref("copyUrlPasswordConfirmed", true);
    const p = await ipc.profiles.get("p-local-pg");
    const profile: ConnectionProfile = {
      ...p,
      color: "green",
      ssh: {
        host: "bastion.example.com",
        port: 22,
        user: "ops",
        auth: "password",
        keyPath: null,
        localPort: null,
        keepaliveSecs: null,
      },
      options: { ...p.options, tag: "staging" },
    };
    vi.spyOn(ipc.profiles, "get").mockResolvedValue(profile);
    render(<SafetyHost />);
    expect(await copyConnectionUrl(profile, false)).toBe(true);
    expect(clipboard).toMatch(
      /^postgresql\+ssh:\/\/ops@bastion\.example\.com\/[^:@/]+@[^/]+\/[^?]*\?name=.*&statusColor=25984D&env=staging$/,
    );
    expect(clipboard).not.toContain("secret");
    // With the password: the database's, in the inner part.
    expect(await copyConnectionUrl(profile, true)).toBe(true);
    expect(clipboard).toMatch(/^postgresql\+ssh:\/\/ops@bastion\.example\.com\/[^:@/]+:secret@/);
  });

  it("confirms the first time, then copies the URL with the password", async () => {
    const profile = await localPg();
    render(<SafetyHost />);
    const first = copyConnectionUrl(profile, true);
    const alert = await screen.findByRole("alertdialog", { name: COPY_WITH_PASSWORD_TITLE });
    fireEvent.click(within(alert).getByRole("button", { name: "Copy" }));
    expect(await first).toBe(true);
    expect(withPassword(clipboard)).toBe(true);
    expect(getPrefs().copyUrlPasswordConfirmed).toBe(true);

    // Not asked again.
    clipboard = null;
    expect(await copyConnectionUrl(profile, true)).toBe(true);
    expect(withPassword(clipboard)).toBe(true);
  });

  it("copies nothing when the confirmation is cancelled", async () => {
    const reveal = vi.spyOn(ipc.security, "revealPassword");
    const profile = await localPg();
    render(<SafetyHost />);
    const run = copyConnectionUrl(profile, true);
    const alert = await screen.findByRole("alertdialog", { name: COPY_WITH_PASSWORD_TITLE });
    fireEvent.click(within(alert).getByRole("button", { name: "Cancel" }));
    expect(await run).toBe(false);
    expect(reveal).not.toHaveBeenCalled();
    expect(clipboard).toBeNull();
  });

  it("at a Safe level asks for the connection's password first", async () => {
    setPref("copyUrlPasswordConfirmed", true);
    const profile = await localPg("safeNonSelect");
    render(<SafetyHost />);
    const run = copyConnectionUrl(profile, true);
    const dialog = await screen.findByRole("alertdialog", {
      name: "Enter the password of Local Postgres to continue",
    });
    expect(dialog).toHaveTextContent("Copy as URL includes the connection's password.");
    expect(within(dialog).queryByRole("button", { name: "Use Touch ID" })).toBeNull();
    const field = within(dialog).getByLabelText("Password");
    fireEvent.change(field, { target: { value: "nope" } });
    fireEvent.submit(field.closest("form")!);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Wrong password");
    expect(clipboard).toBeNull();
    fireEvent.change(field, { target: { value: "secret" } });
    fireEvent.submit(field.closest("form")!);
    expect(await run).toBe(true);
    expect(withPassword(clipboard)).toBe(true);
  });

  it("with Unlock Safe Mode with Touch ID, Touch ID is offered (once on its own) and enough", async () => {
    setPref("copyUrlPasswordConfirmed", true);
    setPref("safeModeTouchId", true);
    const auth = vi.spyOn(ipc.security, "authenticate").mockResolvedValue(true);
    const profile = await localPg();
    render(<SafetyHost />);
    expect(await copyConnectionUrl(profile, true)).toBe(true);
    expect(auth).toHaveBeenCalledTimes(1);
    expect(auth.mock.calls[0]![0]).toContain("Local Postgres");
    expect(withPassword(clipboard)).toBe(true);
  });

  it("a cancelled Touch ID leaves the typed password and the button", async () => {
    setPref("safeModeTouchId", true);
    vi.spyOn(ipc.security, "authenticate").mockResolvedValue(false);
    const profile = await localPg("safe");
    setPref("copyUrlPasswordConfirmed", true);
    render(<SafetyHost />);
    const run = copyConnectionUrl(profile, true);
    const dialog = await screen.findByRole("alertdialog", {
      name: "Enter the password of Local Postgres to continue",
    });
    const touchId = await within(dialog).findByRole("button", { name: "Use Touch ID" });
    await waitFor(() => expect(touchId).toBeEnabled());
    const field = within(dialog).getByLabelText("Password");
    fireEvent.change(field, { target: { value: "secret" } });
    fireEvent.submit(field.closest("form")!);
    expect(await run).toBe(true);
  });

  it("copies the plain URL when there is no password to carry", async () => {
    const reveal = vi.spyOn(ipc.security, "revealPassword");
    const p = await ipc.profiles.get("p-local-pg");
    const profile = { ...p, options: { ...p.options, passwordMode: "none" as const } };
    render(<SafetyHost />);
    expect(await copyConnectionUrl(profile, true)).toBe(true);
    expect(reveal).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
