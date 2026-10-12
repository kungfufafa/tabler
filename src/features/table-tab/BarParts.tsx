import type { ComponentProps, ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ActionId } from "@/features/palette/actions";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

/**
 * The 32px strip under a table tab's content (TablePlus's bottom bar): the Data | Structure switch
 * on the left, the view's own controls after it. Both views of a table tab use it, so switching
 * between them keeps the switch in place. A size container: parts drop their labels when narrow.
 */
export function BottomBarFrame({
  className,
  children,
  ...props
}: ComponentProps<"div"> & { children: ReactNode }) {
  return (
    <div
      role="toolbar"
      className={cn(
        "border-border bg-surface-2 text-muted-foreground @container flex h-8 shrink-0 items-center gap-1 overflow-hidden border-t px-2 text-xs",
        className,
      )}
      {...props}
    >
      {children}
    </div>
  );
}

/** A thin vertical rule between groups of bottom-bar controls. */
export function BarSeparator({ className }: { className?: string }) {
  return <span aria-hidden className={cn("bg-border mx-0.5 h-4 w-px shrink-0", className)} />;
}

/** An action's primary shortcut, styled for a tooltip (nothing when it has none). */
export function TooltipKeys({ id }: { id: ActionId }) {
  const keys = shortcutFor(id);
  return keys ? (
    <Kbd combo={keys} className="border-border/40 bg-transparent text-inherit" />
  ) : null;
}

/** Tooltip text plus the shortcut of `action`, if any. */
export function BarTooltip({
  label,
  action,
  children,
}: {
  label: ReactNode;
  action?: ActionId;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {action && <TooltipKeys id={action} />}
      </TooltipContent>
    </Tooltip>
  );
}

/** A small icon-only bottom-bar button with a tooltip naming it (and its shortcut). */
export function BarIconButton({
  label,
  tooltip,
  action,
  className,
  children,
  ...props
}: Omit<ComponentProps<typeof Button>, "aria-label" | "variant" | "size"> & {
  /** The accessible name (and the tooltip, unless `tooltip` says more). */
  label: string;
  tooltip?: ReactNode;
  action?: ActionId;
}) {
  return (
    <BarTooltip label={tooltip ?? label} action={action}>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label={label}
        className={cn("text-muted-foreground hover:text-foreground", className)}
        {...props}
      >
        {children}
      </Button>
    </BarTooltip>
  );
}
