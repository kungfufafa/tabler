import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FormField } from "./FormField";

describe("FormField", () => {
  it("labels the control and shows the hint under it", () => {
    render(
      <FormField label="Name" htmlFor="db-name" hint="Letters and digits">
        <input id="db-name" />
      </FormField>,
    );
    expect(screen.getByLabelText("Name")).toBe(document.getElementById("db-name"));
    const hint = screen.getByText("Letters and digits");
    expect(hint.id).toBe("db-name-hint");
    expect(hint).toHaveClass("text-muted-foreground", "text-xs");
    expect(screen.getByText("Name")).toHaveClass("text-muted-foreground", "text-xs", "font-medium");
  });

  it("replaces the hint with an alert while there is an error", () => {
    render(
      <FormField label="Name" htmlFor="db-name" hint="Letters and digits" error="Name is taken">
        <input id="db-name" />
      </FormField>,
    );
    expect(screen.queryByText("Letters and digits")).toBeNull();
    const error = screen.getByRole("alert");
    expect(error).toHaveTextContent("Name is taken");
    expect(error.id).toBe("db-name-error");
    expect(error).toHaveClass("text-error", "text-xs");
  });

  it("puts the label in a right-aligned column when inline", () => {
    const { container } = render(
      <FormField label="Format" htmlFor="fmt" orientation="inline">
        <input id="fmt" />
      </FormField>,
    );
    expect(container.firstElementChild).toHaveClass("grid", "grid-cols-[5.5rem_minmax(0,1fr)]");
    const label = screen.getByText("Format");
    expect(label).toHaveClass("text-right", "pt-1.5");
    expect(label).not.toHaveClass("font-medium");
    expect(screen.getByLabelText("Format")).toBe(document.getElementById("fmt"));
  });
});
