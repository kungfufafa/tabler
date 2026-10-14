import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { GroupIcon } from "./badges";

const PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

describe("GroupIcon", () => {
  it("draws the icon, the folder as a fallback, or nothing", () => {
    const { container, rerender } = render(<GroupIcon icon="home" />);
    expect(container.querySelector("svg.lucide-house")).not.toBeNull();
    expect(container.querySelector("img")).toBeNull();
    rerender(<GroupIcon icon={null} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<GroupIcon icon={null} fallback />);
    expect(container.querySelector("svg.lucide-folder")).not.toBeNull();
  });

  it("shows the group's image instead of its icon, at the icon's size", () => {
    const { container } = render(<GroupIcon icon="home" image={PNG} className="size-3" />);
    const img = container.querySelector("img")!;
    expect(img).toHaveAttribute("src", PNG);
    expect(img).toHaveAttribute("alt", "");
    expect(img).toHaveClass("size-3", "rounded-[3px]", "object-contain", "shrink-0");
    expect(img).toHaveAttribute("draggable", "false");
    // The icon is not drawn next to it.
    expect(container.querySelector("svg")).toBeNull();
    // Decorative: nothing to read out.
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("takes the image over the fallback folder too", () => {
    const { container } = render(<GroupIcon icon={null} image={PNG} fallback />);
    expect(container.querySelector("img")).toHaveAttribute("src", PNG);
    expect(container.querySelector("svg")).toBeNull();
  });

  it("never renders what is not a data:image URI", () => {
    for (const image of ["javascript:alert(1)", "https://example.com/logo.png", "", null]) {
      const { container, unmount } = render(<GroupIcon icon="home" image={image} />);
      expect(container.querySelector("img")).toBeNull();
      expect(container.querySelector("svg.lucide-house")).not.toBeNull();
      unmount();
    }
  });
});
