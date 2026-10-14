import type { ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions, type ActionId } from "@/features/palette/actions";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

/** Tooltip with the action's label and its primary shortcut. */
export function ToolbarTip({
  label,
  action,
  side = "bottom",
  children,
}: {
  label: ReactNode;
  action?: ActionId;
  side?: "top" | "bottom" | "left" | "right";
  children: ReactNode;
}) {
  const combo = action ? shortcutFor(action) : undefined;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side={side} className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Small ghost icon button of the toolbar: runs `action` (unless `onClick` is given) and shows its
 * label and shortcut in a tooltip. `pressed` marks a toggle (panel buttons) and highlights it.
 * `text`: a caption beside the icon (Customize Toolbar ▸ Icon and Text).
 */
export function ToolbarButton({
  label,
  action,
  onClick,
  disabled,
  pressed,
  className,
  text,
  children,
}: {
  label: string;
  action: ActionId;
  onClick?: () => void;
  disabled?: boolean;
  pressed?: boolean;
  className?: string;
  text?: string;
  children: ReactNode;
}) {
  return (
    <ToolbarTip label={label} action={action}>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={label}
        aria-pressed={pressed}
        disabled={disabled}
        onClick={onClick ?? (() => void actions.run(action))}
        className={cn(
          "text-muted-foreground hover:text-foreground size-6.5",
          text && "w-auto gap-1 px-1.5 text-[11px] font-medium",
          pressed && "bg-brand-soft text-brand hover:bg-brand-soft hover:text-brand",
          className,
        )}
      >
        {children}
        {text && <span data-slot="toolbar-text">{text}</span>}
      </Button>
    </ToolbarTip>
  );
}
