import { render, screen, within } from "@testing-library/react";
import { Folder } from "lucide-react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DropdownMenuEntries, type MenuEntry } from "./menuModel";

beforeAll(() => {
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.scrollIntoView ??= () => {};
});

function renderMenu(entries: MenuEntry[]) {
  render(
    <DropdownMenu open>
      <DropdownMenuTrigger>Open</DropdownMenuTrigger>
      <DropdownMenuContent>
        <DropdownMenuEntries entries={entries} />
      </DropdownMenuContent>
    </DropdownMenu>,
  );
  return screen.getByRole("menu");
}

describe("menu model", () => {
  it("drops leading, trailing and doubled separators", () => {
    const run = vi.fn();
    const menu = renderMenu([
      { kind: "separator" },
      { kind: "item", label: "A", run },
      { kind: "separator" },
      { kind: "separator" },
      { kind: "item", label: "B", run, disabled: true },
      { kind: "separator" },
    ]);
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((i) => i.textContent?.trim()),
    ).toEqual(["A", "B"]);
    expect(within(menu).getAllByRole("separator")).toHaveLength(1);
    expect(within(menu).getByRole("menuitem", { name: "B" })).toHaveAttribute("data-disabled");
  });

  it("indents the items of a group tree by their level", () => {
    const run = vi.fn();
    const menu = renderMenu([
      { kind: "item", label: "Clients", icon: Folder, run },
      { kind: "item", label: "Acme", icon: Folder, run, indent: 1 },
      { kind: "item", label: "Prod", icon: Folder, run, indent: 2 },
    ]);
    const item = (name: string) => within(menu).getByRole("menuitem", { name });
    expect(item("Clients").getAttribute("style") ?? "").not.toContain("padding-left");
    expect(item("Acme")).toHaveStyle({ paddingLeft: "22px" });
    expect(item("Prod")).toHaveStyle({ paddingLeft: "36px" });
    item("Prod").click();
    expect(run).toHaveBeenCalledTimes(1);
  });
});
