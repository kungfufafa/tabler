import { useQueryClient } from "@tanstack/react-query";
import { ArrowUpFromLine, CircleX, Eye, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { openPendingSheet, runObjectCommit } from "@/features/objects/pendingActions";
import { useObjectOpsCommitting } from "@/features/objects/pendingOps";
import { actions, type ActionId } from "@/features/palette/actions";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { ToolbarTip } from "./ToolbarButton";
import { useActiveObjectOpCount, useActiveTabPendingCount } from "./useActivePendingCount";

function Segment({
  label,
  action,
  icon: Icon,
  disabled,
  className,
  children,
  onClick,
}: {
  label: string;
  action: ActionId;
  icon: LucideIcon;
  disabled: boolean;
  className?: string;
  children?: ReactNode;
  /** Instead of running `action`. */
  onClick?: () => void;
}) {
  const button = (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onClick={onClick ?? (() => void actions.run(action))}
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
 * table's rows or structure, or the edits in a query tab's results) and of the active
 * connection's pending item operations (sidebar Truncate… / Delete…), in one segmented group.
 * Commit runs the tab's commit, then the operations (the operations alone when the tab has
 * nothing staged); Discard drops both; Preview shows both (in the operations' sheet when there are
 * any). Disabled while nothing is staged, and Discard / Commit while the operations are being
 * committed; Commit shows the pending count.
 */
export function ActionControl() {
  const qc = useQueryClient();
  const tabPending = useActiveTabPendingCount();
  const objectOps = useActiveObjectOpCount();
  const pending = tabPending + objectOps;
  const connectionId = useConnectionsStore((s) => s.activeId);
  const committing = useObjectOpsCommitting(connectionId);
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
      <Segment
        label="Preview SQL"
        action="grid.preview"
        icon={Eye}
        disabled={none}
        onClick={
          objectOps > 0 && connectionId !== null ? () => openPendingSheet(connectionId) : undefined
        }
      />
      <Segment
        label={none ? "Commit changes" : `Commit ${changes}`}
        action="grid.commit"
        icon={ArrowUpFromLine}
        disabled={none || committing}
        className={cn(!none && "text-brand hover:text-brand")}
        onClick={
          tabPending === 0 && objectOps > 0 && connectionId !== null
            ? () => void runObjectCommit(qc, connectionId)
            : undefined
        }
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
