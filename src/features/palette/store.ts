import { create } from "zustand";

/**
 * What the palette lists: everything (Open Anything, ⌘P), only the active connection's databases
 * and schemas (⌘K), only the saved connections (⇧⌘K), or every object of the active database
 * across all of its schemas, loaded or not (the sidebar search menu's "Search in Database…").
 */
export type PaletteMode = "all" | "databases" | "connections" | "objects";

interface PaletteState {
  open: boolean;
  mode: PaletteMode;
  /** Bumped on every open so each opening starts from an empty query. */
  session: number;
  /** The query an opening starts with (`openPalette`; empty otherwise). */
  initialQuery: string;
  /** Profiles with a connect started from the palette still in flight. */
  connecting: readonly string[];
  setOpen: (open: boolean) => void;
  /** Toggles the palette; opening in another mode while open switches the mode instead. */
  toggle: (mode?: PaletteMode) => void;
  /** Opens (or reopens) the palette in `mode` with `query` typed in. */
  openPalette: (mode: PaletteMode, query?: string) => void;
  /** Marks a profile's connect as started; false when one is already in flight. */
  beginConnect: (profileId: string) => boolean;
  endConnect: (profileId: string) => void;
}

/** Open state of the command palette: `palette.open` (⌘P), `palette.databases` (⌘K), `palette.connections` (⇧⌘K). */
export const usePaletteStore = create<PaletteState>()((set, get) => ({
  open: false,
  mode: "all",
  session: 0,
  initialQuery: "",
  connecting: [],
  // Closing keeps the mode: the list must not change while the dialog animates out.
  setOpen: (open) =>
    set((s) =>
      open === s.open
        ? s
        : open
          ? { open, mode: "all", session: s.session + 1, initialQuery: "" }
          : { open },
    ),
  toggle: (mode = "all") =>
    set((s) => {
      if (!s.open) return { open: true, mode, session: s.session + 1, initialQuery: "" };
      if (s.mode !== mode) return { mode, session: s.session + 1, initialQuery: "" };
      return { open: false };
    }),
  openPalette: (mode, query = "") =>
    set((s) => ({ open: true, mode, session: s.session + 1, initialQuery: query })),
  beginConnect: (profileId) => {
    if (get().connecting.includes(profileId)) return false;
    set((s) => ({ connecting: [...s.connecting, profileId] }));
    return true;
  },
  endConnect: (profileId) =>
    set((s) =>
      s.connecting.includes(profileId)
        ? { connecting: s.connecting.filter((id) => id !== profileId) }
        : s,
    ),
}));
