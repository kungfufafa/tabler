import { toast } from "sonner";
import { openSettings } from "@/features/settings/store";
import { ipc } from "@/lib/ipc";
import { lockNow, useLockStore } from "./store";

/**
 * Window ▸ Lock Tabler (⌃⌘L): every window locks. Without a passcode it says where to set one.
 */
export function lockTabler(): void {
  if (!useLockStore.getState().enabled) {
    toast.info("Set a passcode to lock Tabler", {
      id: "lock-no-passcode",
      description: "Settings ▸ Security ▸ Enable Passcode.",
      action: { label: "Settings…", onClick: () => openSettings("security") },
    });
    return;
  }
  lockNow("user");
  // The other windows lock through the backend's broadcast (this one already is).
  ipc.security.lockApp().catch(() => undefined);
}
