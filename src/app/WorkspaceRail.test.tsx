import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ipc, type ConnectionInfo } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { WorkspaceRail } from "./WorkspaceRail";

const info = (connectionId: string, database: string, profileId = "p1"): ConnectionInfo => ({
  connectionId,
  profileId,
  name: "Local PG",
  color: "green",
  driver: "postgres",
  serverVersion: "PostgreSQL 17.6",
  currentDatabase: database,
  currentUser: "me",
  tunnel: null,
  readOnly: false,
});

function renderRail() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <WorkspaceRail />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const rail = () => screen.getByRole("navigation", { name: "Open connections" });

describe("WorkspaceRail database icons", () => {
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useUiStore.setState({
      view: "workspace",
      railOrder: [],
      railShowDatabase: false,
      railShowConnection: false,
    });
    // The rail shows from the second workspace on: one more, named apart from the ones under test.
    useConnectionsStore.getState().add({ ...info("other", "scratch", "p9"), name: "Other" });
    vi.restoreAllMocks();
  });

  it("shows one icon per workspace: the database's initials, in the saved order", () => {
    useConnectionsStore.getState().add(info("a", "tabler_dev"));
    useConnectionsStore.getState().add(info("b", "shop"));
    useUiStore.setState({ railOrder: ["p1\u0000shop"] });
    renderRail();
    const buttons = within(rail()).getAllByRole("button", { name: /Local PG/ });
    expect(buttons.map((b) => b.getAttribute("aria-label"))).toEqual([
      "shop (Local PG)",
      "tabler_dev (Local PG)",
    ]);
    expect(buttons[1]).toHaveTextContent("TD");
    expect(buttons[1]).not.toHaveTextContent("tabler_dev");
    expect(buttons[0]).toHaveAttribute("aria-current", "true");
  });

  it("marks a tagged, read-only workspace on its icon", async () => {
    const prod = SEED_PROFILES.find((p) => p.id === "p-prod-pg")!;
    vi.spyOn(ipc.profiles, "list").mockResolvedValue([prod]);
    useConnectionsStore.getState().add({ ...info("a", "tabler_dev", prod.id), readOnly: true });
    renderRail();
    const button = await within(rail()).findByRole("button", {
      name: "tabler_dev (Local PG), Production, read-only",
    });
    expect(button.querySelector("[data-slot=rail-tag]")).toHaveTextContent("PROD");
    expect(button.querySelector("[data-slot=rail-read-only]")).not.toBeNull();
  });

  it("shows no badge for a local workspace", () => {
    useConnectionsStore.getState().add(info("a", "tabler_dev"));
    renderRail();
    const button = within(rail()).getByRole("button", { name: "tabler_dev (Local PG)" });
    expect(button.querySelector("[data-slot=rail-tag]")).toBeNull();
    expect(button.querySelector("[data-slot=rail-read-only]")).toBeNull();
  });

  it("is icons alone (TablePlus); the right-click menu adds the database or connection name", () => {
    useConnectionsStore.getState().add(info("a", "tabler_dev"));
    renderRail();
    const button = within(rail()).getByRole("button", { name: /tabler_dev/ });
    expect(button.querySelector("[data-slot=rail-database]")).toBeNull();
    expect(button.querySelector("[data-slot=rail-connection]")).toBeNull();
    fireEvent.contextMenu(button);
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "Show Database Name" }));
    expect(useUiStore.getState().railShowDatabase).toBe(true);
    expect(button.querySelector("[data-slot=rail-database]")).not.toBeNull();
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "Show Connection Name" }));
    expect(useUiStore.getState().railShowConnection).toBe(true);
    expect(button.querySelector("[data-slot=rail-connection]")).not.toBeNull();
  });

  it("opens a workspace in a new window", async () => {
    const open = vi.spyOn(ipc.windows, "open").mockResolvedValue("w-2");
    useConnectionsStore.getState().add(info("a", "shop"));
    renderRail();
    fireEvent.contextMenu(within(rail()).getByRole("button", { name: /shop/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open in New Window" }));
    await vi.waitFor(() =>
      expect(open).toHaveBeenCalledWith({
        profileId: "p1",
        database: "shop",
        schema: null,
        tab: null,
      }),
    );
  });
});
