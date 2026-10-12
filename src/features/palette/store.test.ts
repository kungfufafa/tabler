import { beforeEach, describe, expect, it } from "vitest";
import { usePaletteStore } from "./store";

describe("palette store", () => {
  beforeEach(() => usePaletteStore.setState({ open: false, mode: "all", session: 0 }));

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

  it("opens in a mode, switches modes while open and closes on the same mode", () => {
    const s = usePaletteStore.getState();
    s.toggle("databases");
    expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "databases", session: 1 });
    // Another mode while open: a fresh list in that mode, still open.
    s.toggle("connections");
    expect(usePaletteStore.getState()).toMatchObject({
      open: true,
      mode: "connections",
      session: 2,
    });
    s.toggle("connections");
    // Closing keeps the mode, so the list does not change while the dialog animates out.
    expect(usePaletteStore.getState()).toMatchObject({
      open: false,
      mode: "connections",
      session: 2,
    });
    // A plain opening (titlebar button, setOpen) is Open Anything.
    s.setOpen(true);
    expect(usePaletteStore.getState()).toMatchObject({ open: true, mode: "all", session: 3 });
    s.toggle();
    expect(usePaletteStore.getState().open).toBe(false);
  });
});
