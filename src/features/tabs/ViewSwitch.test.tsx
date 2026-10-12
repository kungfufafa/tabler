import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { useTabsStore } from "@/stores/tabs";
import { ViewSwitch } from "./ViewSwitch";

const kind = (id: string) => useTabsStore.getState().tabs.find((t) => t.id === id)?.kind;

describe("ViewSwitch", () => {
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
  });

  it("is a segmented control: one bordered group, the selected segment filled with the accent", () => {
    const id = useTabsStore.getState().openTable("c1", { schema: "public", name: "users" });
    render(<ViewSwitch tabId={id} view="table" />);
    const group = screen.getByRole("radiogroup", { name: "Table view" });
    expect(group).toHaveClass("border", "rounded-md", "h-5.5", "divide-x");

    const data = screen.getByRole("radio", { name: "Data" });
    const structure = screen.getByRole("radio", { name: "Structure" });
    expect(data).toHaveAttribute("aria-checked", "true");
    expect(data).toHaveClass("bg-primary", "text-primary-foreground");
    expect(structure).toHaveAttribute("aria-checked", "false");
    expect(structure).toHaveClass("text-muted-foreground");
    expect(structure).not.toHaveClass("bg-primary");
  });

  it("switches the tab's view and names the shortcuts", () => {
    const id = useTabsStore.getState().openTable("c1", { schema: "public", name: "users" });
    render(<ViewSwitch tabId={id} view="table" />);
    const structure = screen.getByRole("radio", { name: "Structure" });
    expect(structure).toHaveAttribute(
      "title",
      `Structure (${formatCombo(shortcutFor("view.structure")!)})`,
    );
    expect(screen.getByRole("radio", { name: "Data" })).toHaveAttribute(
      "title",
      `Data (${formatCombo(shortcutFor("view.data")!)})`,
    );
    fireEvent.click(structure);
    expect(kind(id)).toBe("structure");
  });
});
