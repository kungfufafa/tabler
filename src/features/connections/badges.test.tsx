import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { KINDS } from "@/features/drivers/catalog";
import { DATABASE_LOGOS } from "@/features/drivers/logos";
import { DriverBadge, GroupIcon } from "./badges";

describe("DriverBadge", () => {
  it("shows a bundled logo for every database instead of an abbreviation", () => {
    for (const { kind, label } of KINDS) {
      const { container, unmount } = render(<DriverBadge kind={kind} color="blue" />);
      expect(container.firstChild).toHaveAttribute("title", label);
      expect(container).toHaveTextContent("");
      const image = container.querySelector("img");
      if (image) expect(image).toHaveAttribute("src", DATABASE_LOGOS[kind]);
      else
        expect(container.querySelector('[data-slot="database-logo"]')).toHaveStyle({
          maskImage: `url("${DATABASE_LOGOS[kind]}")`,
        });
      unmount();
    }
  });

  it("uses the flavor logo and replaces it with a spinner while connecting", () => {
    const { container, rerender } = render(
      <DriverBadge driver="mysql" mysqlVersion="mariadb" color="purple" size="lg" />,
    );
    expect(container.firstChild).toHaveAttribute("title", "MariaDB");
    expect(container.querySelector('[data-slot="database-logo"]')).toHaveClass("size-8");
    rerender(<DriverBadge driver="mysql" color="purple" busy />);
    expect(container.querySelector('[data-slot="database-logo"]')).toBeNull();
    expect(container.querySelector("svg.animate-spin")).not.toBeNull();
  });
});

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
