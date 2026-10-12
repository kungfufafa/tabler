import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { stopTabRename } from "./renameStore";
import { TabStrip } from "./TabStrip";

const users = { schema: "public", name: "users" };

function renderStrip() {
  return render(
    <TooltipProvider>
      <TabStrip />
    </TooltipProvider>,
  );
}

const title = (id: string) => useTabsStore.getState().tabs.find((t) => t.id === id)?.title;

async function nameField() {
  // The field takes focus right after the context menu let go of it.
  const input = await screen.findByRole("textbox", { name: "Tab name" });
  await act(() => new Promise((r) => setTimeout(r, 0)));
  return input;
}

describe("renaming tabs", () => {
  beforeEach(() => {
    useTabsStore.setState({
      tabs: [],
      activeId: null,
      closed: [],
      queryCounter: 0,
      closeRequest: null,
      history: [],
      historyIndex: -1,
    });
    useConnectionsStore.setState({ byId: {}, order: [], activeId: "c1" });
    useUiStore.setState({ view: "workspace" });
    stopTabRename();
  });

  it("renames a query tab in place on double-click", async () => {
    const q = useTabsStore.getState().newQuery("c1");
    renderStrip();
    fireEvent.doubleClick(screen.getByRole("tab", { name: /Query 1/ }));
    const input = await nameField();
    expect(input).toHaveFocus();
    expect(input).toHaveValue("Query 1");
    fireEvent.change(input, { target: { value: "Monthly revenue" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(title(q)).toBe("Monthly revenue");
    expect(screen.queryByRole("textbox", { name: "Tab name" })).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Monthly revenue/ })).toBeInTheDocument();
  });

  it("keeps the name on Esc and ignores a blank one", async () => {
    const q = useTabsStore.getState().newQuery("c1");
    renderStrip();
    fireEvent.doubleClick(screen.getByRole("tab", { name: /Query 1/ }));
    let input = await nameField();
    fireEvent.change(input, { target: { value: "Nope" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(title(q)).toBe("Query 1");
    fireEvent.doubleClick(screen.getByRole("tab", { name: /Query 1/ }));
    input = await nameField();
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.blur(input);
    expect(title(q)).toBe("Query 1");
  });

  it("renames any tab from the context menu, and lists Close All Tabs", async () => {
    const t = useTabsStore.getState().openTable("c1", users);
    renderStrip();
    fireEvent.contextMenu(screen.getByRole("tab", { name: /users/ }));
    expect(screen.getByRole("menuitem", { name: /Close All Tabs/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename Tab…" }));
    const input = await nameField();
    fireEvent.change(input, { target: { value: "People" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(title(t)).toBe("People");
  });

  it("does not rename a table tab on double-click (that keeps a preview open)", () => {
    const t = useTabsStore.getState().openTable("c1", users, "table", { preview: true });
    renderStrip();
    fireEvent.doubleClick(screen.getByRole("tab", { name: /users/ }));
    expect(screen.queryByRole("textbox", { name: "Tab name" })).not.toBeInTheDocument();
    const tab = useTabsStore.getState().tabs.find((x) => x.id === t);
    expect(tab?.kind !== "query" && tab?.preview).toBeFalsy();
  });
});
