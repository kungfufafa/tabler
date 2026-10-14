import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { resetMockPlugins } from "@/lib/ipc/mock/plugins";
import { getPrefs, resetPrefsForTests } from "@/stores/prefs";
import { PluginManager } from "./PluginManager";
import { PluginPrompts } from "./PluginPrompts";
import { openPluginManager, resetPluginsStoreForTests, usePluginsStore } from "./store";

beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
  resetMockPlugins();
});

async function openManager() {
  render(
    <>
      <PluginManager />
      <PluginPrompts />
    </>,
  );
  act(() => openPluginManager());
  await screen.findByText("Row Counter");
}

const rowOf = (name: string) => screen.getByText(name).closest<HTMLElement>("[role='option']")!;

describe("Manage Plugins…", () => {
  it("lists the built-in and installed plugins, the broken ones and the native features", async () => {
    await openManager();
    expect(within(rowOf("SQL Formatter")).getByText("Built-in")).toBeInTheDocument();
    expect(within(rowOf("SQL Formatter")).getByText("SQL Format")).toBeInTheDocument();
    expect(screen.getByText("Sandbox Demo")).toBeInTheDocument();
    expect(screen.getByText(/needs at least one command/)).toBeInTheDocument();
    expect(screen.getByText("Dump Table")).toBeInTheDocument();
    expect(screen.getByText("Diagram")).toBeInTheDocument();
  });

  it("asks for permission before enabling a third-party plugin", async () => {
    const user = userEvent.setup();
    await openManager();
    const toggle = within(rowOf("Row Counter")).getByRole("switch");
    expect(toggle).not.toBeChecked();
    await user.click(toggle);
    const ask = await screen.findByRole("alertdialog");
    expect(within(ask).getByText(/Enable “Row Counter”\?/)).toBeInTheDocument();
    expect(within(ask).getByText(/Run SQL on the connection/)).toBeInTheDocument();
    await user.click(within(ask).getByRole("button", { name: "Enable" }));
    await waitFor(() =>
      expect(getPrefs().pluginStates["com.example.row-counter"]).toEqual({
        enabled: true,
        granted: true,
      }),
    );
    expect(within(rowOf("Row Counter")).getByRole("switch")).toBeChecked();
  });

  it("removes only installed plugins, after a confirmation", async () => {
    const user = userEvent.setup();
    await openManager();
    const remove = screen.getByRole("button", { name: /Remove/ });
    await user.click(rowOf("SQL Formatter"));
    expect(remove).toBeDisabled();
    await user.click(rowOf("Sandbox Demo"));
    expect(remove).toBeEnabled();
    await user.click(remove);
    const confirm = await screen.findByRole("alertdialog");
    await user.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(usePluginsStore.getState().list?.plugins.map((p) => p.manifest.name)).toEqual([
        "Row Counter",
      ]),
    );
  });
});
