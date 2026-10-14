import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { accentColor, applyAccent, hexForeground, isHexColor } from "@/lib/theme";
import { parseHex } from "./colors";
import { CustomColorSwatch } from "./CustomColorSwatch";

afterEach(() => applyAccent(null));

describe("custom connection colours (colour picker + Hex)", () => {
  it("reads the Hex field", () => {
    expect(parseHex("#12AB34")).toBe("#12ab34");
    expect(parseHex("12ab34")).toBe("#12ab34");
    expect(parseHex(" #fa3 ")).toBe("#ffaa33");
    expect(parseHex("#12ab3")).toBeNull();
    expect(parseHex("red")).toBeNull();
  });

  it("theme: hex colours work where preset names do", () => {
    expect(isHexColor("#12ab34")).toBe(true);
    expect(accentColor("#12ab34")).toBe("#12ab34");
    expect(accentColor("red")).toMatch(/^oklch/);
    expect(accentColor("nope")).toBe(accentColor("blue"));
    expect(hexForeground("#ffff00")).toMatch(/0\.2/);
    expect(hexForeground("#202060")).toMatch(/0\.99/);
    const root = document.documentElement;
    applyAccent("#12ab34");
    expect(root.dataset.accent).toBe("custom");
    expect(root.style.getPropertyValue("--brand")).toBe("#12ab34");
    applyAccent("red");
    expect(root.dataset.accent).toBe("red");
    expect(root.style.getPropertyValue("--brand")).toBe("");
    applyAccent(null);
    expect(root.dataset.accent).toBeUndefined();
  });

  it("Custom… opens a colour picker and a Hex field", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(<CustomColorSwatch value="blue" onChange={onChange} />);
    const swatch = screen.getByRole("radio", { name: "Custom…" });
    expect(swatch).toHaveAttribute("aria-checked", "false");
    await user.click(swatch);
    await user.type(screen.getByLabelText("Hex"), "#12AB34{Enter}");
    expect(onChange).toHaveBeenCalledWith("#12ab34");
    rerender(<CustomColorSwatch value="#12ab34" onChange={onChange} />);
    expect(screen.getByRole("radio", { name: "Custom…" })).toHaveAttribute("aria-checked", "true");
  });

  it("does not pull the picker back to a saved colour while it is being dragged", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(<CustomColorSwatch value="#aaaaaa" onChange={onChange} />);
    await user.click(screen.getByRole("radio", { name: "Custom…" }));
    // The picker starts from the saved colour.
    expect(screen.getByLabelText("Colour picker")).toHaveValue("#aaaaaa");
    fireEvent.change(screen.getByLabelText("Colour picker"), { target: { value: "#303030" } });
    expect(onChange).toHaveBeenLastCalledWith("#303030");
    // The save of an earlier step of the drag finishes: its colour comes back as `value`.
    rerender(<CustomColorSwatch value="#101010" onChange={onChange} />);
    expect(screen.getByLabelText("Colour picker")).toHaveValue("#303030");
    expect(screen.getByLabelText("Hex")).toHaveValue("#303030");
    // Closed and opened again, it starts from the saved colour once more.
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByLabelText("Colour picker")).toBeNull());
    await user.click(screen.getByRole("radio", { name: "Custom…" }));
    expect(screen.getByLabelText("Colour picker")).toHaveValue("#101010");
    expect(screen.getByLabelText("Hex")).toHaveValue("#101010");
  });
});
