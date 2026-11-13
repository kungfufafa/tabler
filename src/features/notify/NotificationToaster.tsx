import { Check, CircleX, Info, TriangleAlert, X } from "lucide-react";
import { Toaster } from "@/components/ui/sonner";
import "./notification-toaster.css";

const offset = { left: 16, right: 16, bottom: 32 };

export function NotificationToaster() {
  return (
    <Toaster
      className="tabler-toaster"
      position="bottom-right"
      offset={offset}
      mobileOffset={offset}
      gap={8}
      toastOptions={{ closeButtonAriaLabel: "Dismiss notification" }}
      icons={{
        success: <Check size={16} aria-hidden="true" />,
        error: <CircleX size={16} aria-hidden="true" />,
        warning: <TriangleAlert size={16} aria-hidden="true" />,
        info: <Info size={16} aria-hidden="true" />,
        close: <X size={16} aria-hidden="true" />,
      }}
    />
  );
}
