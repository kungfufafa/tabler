import { create } from "zustand";

interface PaletteState {
  open: boolean;
  /** Bumped on every open so each opening starts from an empty query. */
  session: number;
  /** Profiles with a connect started from the palette still in flight. */
  connecting: readonly string[];
  setOpen: (open: boolean) => void;
  toggle: () => void;
  /** Marks a profile's connect as started; false when one is already in flight. */
  beginConnect: (profileId: string) => boolean;
  endConnect: (profileId: string) => void;
}

/** Open state of the command palette; `palette.open` (⌘K / ⌘P) toggles it. */
export const usePaletteStore = create<PaletteState>()((set, get) => ({
  open: false,
  session: 0,
  connecting: [],
  setOpen: (open) =>
    set((s) => (open === s.open ? s : { open, session: open ? s.session + 1 : s.session })),
  toggle: () => set((s) => ({ open: !s.open, session: s.open ? s.session : s.session + 1 })),
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
