import { create } from "zustand";
import type { ActionId } from "@/features/palette/actions";

/**
 * What the toolbar's Run button does: run the statement under the cursor (or the selection), like
 * ⌘↩, or everything, like ⇧⌘↩. One choice for the whole app, kept in localStorage.
 */
export type RunButtonMode = "current" | "all";

export const RUN_BUTTON_KEY = "tabler.runButton";
const DEFAULT_MODE: RunButtonMode = "current";

/** The action each mode runs (its shortcut is shown next to it). */
export const RUN_BUTTON_ACTION: Record<RunButtonMode, ActionId> = {
  current: "query.run-current",
  all: "query.run",
};

function readMode(): RunButtonMode {
  try {
    const v = globalThis.localStorage?.getItem(RUN_BUTTON_KEY);
    return v === "current" || v === "all" ? v : DEFAULT_MODE;
  } catch {
    return DEFAULT_MODE;
  }
}

function writeMode(mode: RunButtonMode): void {
  try {
    globalThis.localStorage?.setItem(RUN_BUTTON_KEY, mode);
  } catch {
    // Storage unavailable: the choice lasts until the window closes.
  }
}

interface RunButtonState {
  mode: RunButtonMode;
  setMode: (mode: RunButtonMode) => void;
}

export const useRunButtonStore = create<RunButtonState>()((set) => ({
  mode: readMode(),
  setMode: (mode) => {
    writeMode(mode);
    set({ mode });
  },
}));
