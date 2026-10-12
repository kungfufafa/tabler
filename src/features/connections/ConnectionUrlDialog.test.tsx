import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ConnectionUrlDialog } from "./ConnectionUrlDialog";

function renderDialog() {
  const onOpenChange = vi.fn();
  const onParsed = vi.fn();
  render(<ConnectionUrlDialog open onOpenChange={onOpenChange} onParsed={onParsed} />);
  return { onOpenChange, onParsed, field: screen.getByRole("textbox", { name: "Connection URL" }) };
}

describe("ConnectionUrlDialog", () => {
  it("needs some text before Continue", () => {
    const { field } = renderDialog();
    expect(field).toHaveFocus();
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });

  it("explains what is wrong inline and stays open", async () => {
    const user = userEvent.setup();
    const { field, onParsed, onOpenChange } = renderDialog();
    await user.type(field, "sqlite://relative.db{Enter}");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Use an absolute path to the database file.",
    );
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(onParsed).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();
    // Editing clears the error.
    await user.type(field, "x");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("hands a valid URL over (password included) and closes", async () => {
    const user = userEvent.setup();
    const { field, onParsed, onOpenChange } = renderDialog();
    await user.type(field, "  mariadb://root:pw@db:3307/shop  ");
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onParsed).toHaveBeenCalledWith({
      values: expect.objectContaining({
        driver: "mysql",
        host: "db",
        port: "3307",
        user: "root",
        database: "shop",
      }),
      password: "pw",
    });
  });
});
