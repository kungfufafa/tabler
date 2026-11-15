import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { formatCombo } from "@/lib/keys/combo";
import { KbdHint } from "./KbdHint";

describe("KbdHint", () => {
  it("draws the keycap in the colour of the surface it sits on", () => {
    const { container } = render(<KbdHint combo="Mod+Enter" />);
    const kbd = container.querySelector("kbd");
    expect(kbd).toHaveTextContent(formatCombo("Mod+Enter"));
    expect(kbd).toHaveClass("border-current/30", "bg-transparent", "text-inherit");
    expect(kbd).not.toHaveClass("text-muted-foreground", "bg-surface", "border-border");
  });
});
