import { X } from "lucide-react";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { useIsWelcomeWindow } from "@/stores/windowShape";
import "./notification-toaster.css";

/** How long a problem stays on screen unless the call says otherwise (hover pauses it). */
export const ERROR_TOAST_MS = 10_000;
export const WARNING_TOAST_MS = 8_000;

/**
 * Errors and warnings get time to be read (and copied); sonner's 4 s default is for "Saved".
 * sonner has no per-type default, so its two helpers are wrapped once; an explicit `duration`
 * still wins.
 */
function lengthenProblemToasts() {
  const t = toast as unknown as Record<string, unknown> & { __tablerLonger?: boolean };
  if (typeof t.error !== "function" || typeof t.warning !== "function" || t.__tablerLonger) return;
  const wrap =
    (fn: (message: unknown, data?: Record<string, unknown>) => unknown, duration: number) =>
    (message: unknown, data?: Record<string, unknown>) =>
      fn(message, { duration, ...data });
  t.error = wrap(t.error as never, ERROR_TOAST_MS);
  t.warning = wrap(t.warning as never, WARNING_TOAST_MS);
  t.__tablerLonger = true;
}
lengthenProblemToasts();

const workspaceOffset = { left: 12, right: 12, bottom: 16 };
const launcherOffset = { ...workspaceOffset, bottom: 68 };

/** `compact`: a window without a status bar to keep clear of (the Settings window). */
export function NotificationToaster({ compact = false }: { compact?: boolean }) {
  const launcher = useUiStore((state) => state.view === "launcher");
  const active = useActiveConnection();
  const welcome = useIsWelcomeWindow();
  const offset = !compact && (launcher || !active || welcome) ? launcherOffset : workspaceOffset;

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
