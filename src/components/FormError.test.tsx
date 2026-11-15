import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { FormError } from "./FormError";

describe("FormError", () => {
  it("announces the message with an icon in the error colour", () => {
    render(<FormError>Could not connect</FormError>);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Could not connect");
    expect(alert).toHaveClass("text-error", "text-xs");
    expect(alert.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("clamps to two lines and keeps the full text in the title", () => {
    render(
      <FormError clamp title="A long server message">
        A long server message
      </FormError>,
    );
    const text = screen.getByText("A long server message");
    expect(text).toHaveClass("line-clamp-2");
    expect(text).toHaveAttribute("title", "A long server message");
  });
});
