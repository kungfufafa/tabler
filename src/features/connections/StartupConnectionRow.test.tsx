import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { getPrefs, resetPrefsForTests } from "@/stores/prefs";
import { StartupConnectionRow } from "./StartupConnectionRow";

beforeAll(async () => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
  await import("@/lib/ipc/mock");
});

beforeEach(() => resetPrefsForTests());

describe("Settings ▸ General ▸ At launch", () => {
  it("chooses a connection to open at startup, and the input form option", async () => {
    const user = userEvent.setup();
    render(<StartupConnectionRow />);
    const select = screen.getByRole("combobox", { name: "At launch" });
    expect(select).toHaveTextContent("Show the welcome screen");
    expect(screen.queryByRole("checkbox", { name: "Prompt the input form" })).toBeNull();
    const pg = SEED_PROFILES[0]!;
    select.focus();
    await user.keyboard("{Enter}");
    await user.click(await screen.findByRole("option", { name: `Open connection ▸ ${pg.name}` }));
    expect(getPrefs()).toMatchObject({ startupAction: "connection", startupProfileId: pg.id });
    await user.click(screen.getByRole("checkbox", { name: "Prompt the input form" }));
    expect(getPrefs().startupPromptForm).toBe(true);
    await waitFor(() => expect(select).toHaveTextContent(pg.name));
  });
});
