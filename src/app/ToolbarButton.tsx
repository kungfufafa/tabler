import type { ComponentProps, ReactNode } from "react";
import { KbdHint } from "@/components/KbdHint";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions, type ActionId } from "@/features/palette/actions";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";

/**
 * Tooltip with the action's label and its primary shortcut, and `disabledReason` under them (why
 * the button is greyed out: "Nothing to commit").
 *
 * `wrap`: the tooltip hangs on a `<span>` around `children` instead of on `children` itself. A
 * disabled button gets no pointer events (`disabled:pointer-events-none`), so a tooltip on the
 * button would never open; the span still gets the hover, and keyboard focus on the button
 * bubbles up to it. The span takes no focus of its own (no tabIndex). `wrapClassName` lays it out
 * (`flex h-full` inside a stretched group).
 */
export function ToolbarTip({
  label,
  action,
  side = "bottom",
  disabledReason,
  wrap = false,
  wrapClassName,
  children,
}: {
  label: ReactNode;
  action?: ActionId;
  side?: "top" | "bottom" | "left" | "right";
  disabledReason?: string;
  wrap?: boolean;
  wrapClassName?: string;
  children: ReactNode;
}) {
  const combo = action ? shortcutFor(action) : undefined;
  const title = (
    <>
      {label}
      {combo && <KbdHint combo={combo} />}
    </>
  );
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {wrap ? (
          <span data-slot="toolbar-tip" className={cn("inline-flex", wrapClassName)}>
            {children}
          </span>
        ) : (
          children
        )}
      </TooltipTrigger>
      {disabledReason ? (
        <TooltipContent side={side} className="flex flex-col items-start gap-0.5">
          <span className="flex items-center gap-1.5">{title}</span>
          <span data-slot="toolbar-tip-reason" className="opacity-70">
            {disabledReason}
          </span>
        </TooltipContent>
      ) : (
        <TooltipContent side={side} className="flex items-center gap-1.5">
          {title}
        </TooltipContent>
      )}
    </Tooltip>
  );
}

/**
 * The buttons inside a capsule shrink to its inner height, and the icon-only ones stay square
 * (a button with a caption, Icon and Text, keeps its width). Descendant rules, so every kind of
 * toolbar button fits without knowing it sits in one.
 */
const GROUP_BUTTONS =
  "[&_[data-size]]:h-6 [&_[data-size=icon-sm]:not(:has([data-slot=toolbar-text]))]:w-6";

/**
 * TablePlus's toolbar capsule: a rounded, bordered run of buttons that belong together (sidebar
 * and menu, lock and Databases / SQL, Metrics / Refresh / Open Anything, the panel switches).
 */
export function ToolbarGroup({ className, children, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="toolbar-group"
      {...props}
      className={cn(
        "border-border bg-surface flex h-7 shrink-0 items-center gap-0.5 rounded-md border p-px",
        GROUP_BUTTONS,
        className,
      )}
    >
      {children}
    </div>
  );
}

/**
 * Small ghost icon button of the toolbar: runs `action` (unless `onClick` is given) and shows its
 * label and shortcut in a tooltip, also while it is disabled (with `disabledReason`, if given).
 * `pressed` marks a toggle (panel buttons) and highlights it. `text`: a caption beside the icon
 * (Customize Toolbar ▸ Icon and Text).
 */
export function ToolbarButton({
  label,
  action,
  onClick,
  disabled,
  disabledReason,
  pressed,
  className,
  text,
  children,
}: {
  label: string;
  action: ActionId;
  onClick?: () => void;
  disabled?: boolean;
  disabledReason?: string;
  pressed?: boolean;
  className?: string;
  text?: string;
  children: ReactNode;
}) {
  // Always wrapped: toggling `disabled` must not remount the button (it would lose focus).
  return (
    <ToolbarTip
      label={label}
      action={action}
      disabledReason={disabled ? disabledReason : undefined}
      wrap
    >
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label={label}
        aria-pressed={pressed}
        disabled={disabled}
        onClick={onClick ?? (() => void actions.run(action))}
        className={cn(
          "text-muted-foreground hover:bg-accent hover:text-foreground dark:hover:bg-accent aria-expanded:bg-accent transition-colors",
          text && "w-auto gap-1 px-2 text-xs font-medium",
          pressed &&
            "bg-brand-soft text-brand hover:bg-brand-soft hover:text-brand dark:hover:bg-brand-soft",
          className,
        )}
      >
        {children}
        {text && <span data-slot="toolbar-text">{text}</span>}
      </Button>
    </ToolbarTip>
  );
}
