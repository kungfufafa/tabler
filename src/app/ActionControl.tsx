import { ArrowUpFromLine, CircleX, Eye, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useCommitting } from "@/features/commit/store";
import { useActiveWorkspaceId } from "@/features/commit/workspace";
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
 * TablePlus's action control: Discard / Preview SQL / Commit of the whole workspace — every
 * pending change of the active connection: the rows staged in any of its table tabs, the edits in
 * its query results, its staged structure, and the sidebar's pending Truncate… / Delete…, all
 * committed together in one transaction (`features/commit/workspace.ts`). Disabled while nothing
 * is pending, and Discard / Commit while a commit runs; Commit shows the pending count.
 */
export function ActionControl() {
  const pending = useActivePendingCount();
  const connectionId = useActiveWorkspaceId();
  const committing = useCommitting(connectionId);
  const none = pending === 0;
  const changes = `${pending} change${pending === 1 ? "" : "s"}`;
  return (
    <div
      role="group"
      aria-label="Staged changes"
      data-slot="action-control"
      className="border-border bg-surface divide-border flex h-6 shrink-0 items-stretch divide-x overflow-hidden rounded-md border"
    >
      <Segment
        label="Discard changes"
        action="grid.discard"
        icon={CircleX}
        disabled={none || committing}
      />
      <Segment label="Preview SQL" action="grid.preview" icon={Eye} disabled={none} />
      <Segment
        label={none ? "Commit changes" : `Commit ${changes}`}
        action="grid.commit"
        icon={ArrowUpFromLine}
        disabled={none || committing}
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
