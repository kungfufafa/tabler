import { beforeEach, describe, expect, it } from "vitest";
import { usePaletteStore } from "./store";

describe("palette store", () => {
  beforeEach(() => usePaletteStore.setState({ open: false, session: 0 }));

  it("starts a new session on every opening, not on closing", () => {
    const s = usePaletteStore.getState();
    s.toggle();
    expect(usePaletteStore.getState()).toMatchObject({ open: true, session: 1 });
    s.toggle();
    expect(usePaletteStore.getState()).toMatchObject({ open: false, session: 1 });
    s.setOpen(true);
    s.setOpen(true);
    expect(usePaletteStore.getState()).toMatchObject({ open: true, session: 2 });
    s.setOpen(false);
    expect(usePaletteStore.getState()).toMatchObject({ open: false, session: 2 });
  });
});
