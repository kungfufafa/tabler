import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { SectionLabel } from "./SectionLabel";

describe("SectionLabel", () => {
  it("is a muted 11px medium heading by default", () => {
    render(<SectionLabel>Connection</SectionLabel>);
    const heading = screen.getByRole("heading", { level: 3, name: "Connection" });
    expect(heading).toHaveClass("text-muted-foreground", "text-xs", "font-medium");
  });

  it("renders the element it is given and keeps extra props", () => {
    render(
      <SectionLabel as="span" id="roles" className="flex-1">
        Roles
      </SectionLabel>,
    );
    const label = screen.getByText("Roles");
    expect(label.tagName).toBe("SPAN");
    expect(label.id).toBe("roles");
    expect(label).toHaveClass("flex-1", "text-xs");
  });
});
