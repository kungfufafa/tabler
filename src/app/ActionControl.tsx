import { ArrowUpFromLine, CircleX, Eye, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { actions, type ActionId } from "@/features/palette/actions";
import { cn } from "@/lib/utils";
import { ToolbarTip } from "./ToolbarButton";
import { useActivePendingCount } from "./useActivePendingCount";

function Segment({
  label,
  action,
  icon: Icon,
  disabled,
  className,
  children,
}: {
  label: string;
  action: ActionId;
  icon: LucideIcon;
  disabled: boolean;
  className?: string;
  children?: ReactNode;
}) {
  const button = (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onClick={() => void actions.run(action)}
      className={cn(
        "text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/50 flex h-full min-w-7 items-center justify-center gap-1 px-1.5 outline-none focus-visible:ring-2 focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-40",
        className,
      )}
    >
      <Icon className="size-3.5 shrink-0" />
      {children}
    </button>
  );
  // Always wrapped (a disabled button gets no pointer events, so no tooltip): toggling `disabled`
  // must not remount the button.
  return (
    <ToolbarTip label={label} action={action}>
      {button}
    </ToolbarTip>
  );
}

/**
 * TablePlus's action control: Discard / Preview SQL / Commit of the visible tab's staged work (a
 * table's rows or structure, or the edits in a query tab's results), in one segmented group.
 * Disabled while nothing is staged; Commit shows the pending count.
 */
export function ActionControl() {
  const pending = useActivePendingCount();
  const none = pending === 0;
  const changes = `${pending} change${pending === 1 ? "" : "s"}`;
  return (
    <div
      role="group"
      aria-label="Staged changes"
      data-slot="action-control"
      className="border-border bg-surface divide-border flex h-6 shrink-0 items-stretch divide-x overflow-hidden rounded-md border"
    >
      <Segment label="Discard changes" action="grid.discard" icon={CircleX} disabled={none} />
      <Segment label="Preview SQL" action="grid.preview" icon={Eye} disabled={none} />
      <Segment
        label={none ? "Commit changes" : `Commit ${changes}`}
        action="grid.commit"
        icon={ArrowUpFromLine}
        disabled={none}
        className={cn(!none && "text-brand hover:text-brand")}
      >
        {!none && (
          <span
            data-slot="pending-badge"
            className="bg-brand text-primary-foreground flex h-3.5 min-w-3.5 items-center justify-center rounded-full px-1 text-[10px] leading-none font-semibold tabular-nums"
          >
            {pending > 999 ? "999+" : pending}
          </span>
        )}
      </Segment>
    </div>
  );
}
