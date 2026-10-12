import { Loader2 } from "lucide-react";
import type { ConnectionTag, Driver } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { DRIVER_SHORT, TAG_COLOR, TAG_LABEL, TINTED, tintStyle } from "./colors";

/** Rounded square with the driver's short label (PG / MY / SQ), tinted with a connection colour. */
export function DriverBadge({
  driver,
  color,
  busy = false,
  size = "md",
  className,
}: {
  driver: Driver;
  color: string | null | undefined;
  /** Shows a spinner instead of the label (connecting). */
  busy?: boolean;
  size?: "md" | "lg";
  className?: string;
}) {
  return (
    <span
      aria-hidden
      style={tintStyle(color)}
      className={cn(
        TINTED,
        "flex shrink-0 items-center justify-center font-mono font-semibold tracking-tight",
        size === "md" ? "size-7 rounded-md text-[10px]" : "size-12 rounded-xl text-sm",
        className,
      )}
    >
      {busy ? <Loader2 className="size-3.5 animate-spin" /> : DRIVER_SHORT[driver]}
    </span>
  );
}

/** Lowercase environment tag pill (`local`, `production`…) in the tag's colour. */
export function TagPill({ tag, className }: { tag: ConnectionTag; className?: string }) {
  return (
    <span
      style={tintStyle(TAG_COLOR[tag])}
      title={`Tag: ${TAG_LABEL[tag]}`}
      className={cn(
        TINTED,
        "inline-flex h-4 shrink-0 items-center rounded-full px-1.5 text-[10px] leading-none font-medium",
        className,
      )}
    >
      {tag}
    </span>
  );
}
