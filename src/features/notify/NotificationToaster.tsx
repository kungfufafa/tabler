import { X } from "lucide-react";
import { Toaster } from "@/components/ui/sonner";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { useIsWelcomeWindow } from "@/stores/windowShape";
import "./notification-toaster.css";

const workspaceOffset = { left: 12, right: 12, bottom: 16 };
const launcherOffset = { ...workspaceOffset, bottom: 68 };

export function NotificationToaster() {
  const launcher = useUiStore((state) => state.view === "launcher");
  const active = useActiveConnection();
  const welcome = useIsWelcomeWindow();
  const offset = launcher || !active || welcome ? launcherOffset : workspaceOffset;

  return (
    <Toaster
      className="tabler-toaster"
      position="bottom-right"
      offset={offset}
      mobileOffset={offset}
      gap={4}
      expand
      toastOptions={{ closeButtonAriaLabel: "Dismiss notification" }}
      icons={{
        success: null,
        error: null,
        warning: null,
        info: null,
        close: <X size={14} aria-hidden="true" />,
      }}
    />
  );
}
