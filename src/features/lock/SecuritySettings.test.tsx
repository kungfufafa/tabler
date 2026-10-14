import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { SecuritySection } from "@/features/settings/sections";
import { ipc } from "@/lib/ipc";
import { resetMockPasscode } from "@/lib/ipc/mock/security";
import { getPrefs, resetPrefsForTests } from "@/stores/prefs";
import { newPasscodeError } from "./passcodeRules";
import { resetLockForTests, useLockStore } from "./store";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});
beforeEach(() => {
  localStorage.clear();
  resetMockPasscode();
  resetPrefsForTests();
  resetLockForTests();
});

function fill(dialog: HTMLElement, label: string, value: string) {
  fireEvent.change(within(dialog).getByLabelText(label), { target: { value } });
}

describe("passcode rules", () => {
  it("needs 4 characters and a matching confirmation", () => {
    expect(newPasscodeError("123", "123")).toMatch(/at least 4/);
    expect(newPasscodeError("1234", "1235")).toBe("The passcodes do not match.");
    expect(newPasscodeError("1234", "1234")).toBeNull();
  });
});

describe("Settings ▸ Security", () => {
  it("has the Touch ID and passcode rows, the passcode ones off until a passcode is set", () => {
    render(<SecuritySection />);
    expect(screen.getByRole("switch", { name: "Unlock Safe Mode with Touch ID" })).toBeEnabled();
    expect(screen.getByRole("switch", { name: "Enable Passcode" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Change Passcode…" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Unlock Passcode with Touch ID" })).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "Auto lock if away for" })).toHaveTextContent(
      "5 minutes",
    );
    fireEvent.click(screen.getByRole("switch", { name: "Unlock Safe Mode with Touch ID" }));
    expect(getPrefs().safeModeTouchId).toBe(true);
  });

  it("enables, changes and turns off the passcode (only a hash is kept)", async () => {
    const set = vi.spyOn(ipc.security, "setPasscode");
    render(<SecuritySection />);
    fireEvent.click(screen.getByRole("switch", { name: "Enable Passcode" }));
    let dialog = await screen.findByRole("dialog", { name: "Enable Passcode" });
    fill(dialog, "Passcode", "1234");
    fill(dialog, "Confirm passcode", "4321");
    fireEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("do not match");
    fill(dialog, "Confirm passcode", "1234");
    fireEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(useLockStore.getState().enabled).toBe(true));
    expect(set).toHaveBeenLastCalledWith(null, "1234");
    expect(JSON.stringify(localStorage)).not.toContain('"1234"');
    expect(await ipc.security.verifyPasscode("1234")).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Change Passcode…" }));
    dialog = await screen.findByRole("dialog", { name: "Change Passcode" });
    fill(dialog, "Current passcode", "0000");
    fill(dialog, "New passcode", "5678");
    fill(dialog, "Confirm passcode", "5678");
    fireEvent.click(within(dialog).getByRole("button", { name: "Change" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Wrong passcode.");
    fill(dialog, "Current passcode", "1234");
    fireEvent.click(within(dialog).getByRole("button", { name: "Change" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(await ipc.security.verifyPasscode("5678")).toBe(true);

    fireEvent.click(screen.getByRole("switch", { name: "Enable Passcode" }));
    dialog = await screen.findByRole("dialog", { name: "Turn Off Passcode" });
    fill(dialog, "Current passcode", "5678");
    fireEvent.click(within(dialog).getByRole("button", { name: "Turn Off" }));
    await waitFor(() => expect(useLockStore.getState().enabled).toBe(false));
    expect((await ipc.security.passcodeStatus()).enabled).toBe(false);
    act(() => undefined);
  });
});
