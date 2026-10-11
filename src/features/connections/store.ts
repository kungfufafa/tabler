import { create } from "zustand";
import type { ConnectionProfile } from "@/lib/ipc";

interface ConnectionDialogState {
  open: boolean;
  /** Profile being edited, or null for a new connection. */
  profile: ConnectionProfile | null;
  setOpen: (open: boolean) => void;
}

/**
 * Open state of the connection manager's New/Edit dialog. Lives outside the Launcher so other
 * entry points (the command palette's "New connection…") can open the form directly.
 */
export const useConnectionDialog = create<ConnectionDialogState>()((set) => ({
  open: false,
  profile: null,
  // Keep `profile` while closing so the dialog's content doesn't change during the exit animation.
  setOpen: (open) => set({ open }),
}));

/** Opens the connection form: a new connection by default, or `profile` to edit it. */
export function openConnectionDialog(profile: ConnectionProfile | null = null): void {
  useConnectionDialog.setState({ open: true, profile });
}
