import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_PREFS, resetPrefsForTests } from "@/stores/prefs";
import { announceFileDone, resetDoneStore, useDoneStore } from "./done";
import { NotifyHost } from "./NotifyHost";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

const notice = {
  title: "Exported 3 rows",
  popupTitle: "Export completed",
  description: "users.csv · 1 KB · 0.1 s",
  path: "/Users/me/Downloads/users.csv",
};

describe("announceFileDone", () => {
  beforeEach(() => {
    resetDoneStore();
    resetPrefsForTests(DEFAULT_PREFS);
  });
  afterEach(() => {
    toast.success.mockReset();
  });

  it("shows the open containing folder popup by default", async () => {
    const user = userEvent.setup();
    render(<NotifyHost />);
    act(() => announceFileDone(notice));
    const popup = await screen.findByRole("alertdialog");
    expect(within(popup).getByText("Export completed")).toBeInTheDocument();
    expect(within(popup).getByText("Exported 3 rows")).toBeInTheDocument();
    expect(within(popup).getByText("users.csv · 1 KB · 0.1 s")).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
    // Show in Finder closes it too (the browser mock reveals nothing).
    await user.click(within(popup).getByRole("button", { name: "Show in Finder" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(useDoneStore.getState().shown).toBeNull();
  });

  it("is a toast with Show in Finder when the preference is off, or nothing to show", () => {
    resetPrefsForTests({ ...DEFAULT_PREFS, exportRevealPopup: false });
    announceFileDone(notice);
    expect(useDoneStore.getState().shown).toBeNull();
    expect(toast.success).toHaveBeenCalledWith("Exported 3 rows", {
      description: notice.description,
      action: { label: "Show in Finder", onClick: expect.any(Function) },
    });
    resetPrefsForTests(DEFAULT_PREFS);
    announceFileDone({ ...notice, path: null });
    expect(useDoneStore.getState().shown).toBeNull();
    expect(toast.success).toHaveBeenLastCalledWith("Exported 3 rows", {
      description: notice.description,
      action: undefined,
    });
  });
});
