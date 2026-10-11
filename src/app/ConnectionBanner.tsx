import { useQueryClient } from "@tanstack/react-query";
import { LoaderCircle, RotateCw, Unplug } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  bannerState,
  reconnectConnection,
  useReconnectInFlight,
} from "@/hooks/useConnectionEvents";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";

/**
 * Warning amber mixed toward the foreground so it stays legible on the tinted bar: ≥ 7.5:1 in light
 * (a smaller warning share; the light amber is pale) and ≥ 8.8:1 in dark.
 */
const TEXT =
  "text-[color-mix(in_oklab,var(--warning)_40%,var(--foreground))] dark:text-[color-mix(in_oklab,var(--warning)_65%,var(--foreground))]";
/** The server's message: a step quieter than the title, still ≥ 5.4:1 in both themes. */
const MESSAGE_TEXT =
  "text-[color-mix(in_oklab,var(--warning)_55%,var(--foreground))] dark:text-[color-mix(in_oklab,var(--warning)_55%,var(--muted-foreground))]";

/** Inline banner shown above the tab content while the active connection is lost or reconnecting. */
export function ConnectionBanner() {
  const active = useActiveConnection();
  const qc = useQueryClient();
  const connectionId = active?.info.connectionId ?? null;
  // Shared per connection, so it survives unmounts and other connections' attempts finishing.
  const busy = useReconnectInFlight(connectionId);
  const state = bannerState(active, busy);
  if (!state || !connectionId) return null;

  const Icon = state.spinning ? LoaderCircle : Unplug;
  return (
    <div
      role="status"
      aria-live="polite"
      data-slot="connection-banner"
      className="border-warning/30 bg-warning/10 text-warning flex h-7 min-w-0 shrink-0 items-center gap-2 border-b pr-1.5 pl-3 text-xs"
    >
      <Icon className={state.spinning ? "size-3.5 shrink-0 animate-spin" : "size-3.5 shrink-0"} />
      {/* Title and message truncate (the message first: it only gets the space the title leaves)
          so the Reconnect button always stays in view, even at the panel's minimum width. */}
      <span
        data-slot="connection-banner-title"
        className={cn("min-w-0 truncate font-medium", TEXT)}
        // At narrow widths the message gets no room; keep it reachable from the title.
        title={state.message ? `${state.title}: ${state.message}` : state.title}
      >
        {state.title}
      </span>
      {state.message && (
        <span
          data-slot="connection-banner-message"
          className={cn("selectable min-w-0 flex-1 truncate", MESSAGE_TEXT)}
          title={state.message}
        >
          {state.message}
        </span>
      )}
      <Button
        variant="outline"
        size="xs"
        className={cn(
          "border-warning/40 hover:bg-warning/15 dark:border-warning/40 dark:hover:bg-warning/15 ml-auto h-5 shrink-0 bg-transparent dark:bg-transparent",
          TEXT,
          "hover:text-[color-mix(in_oklab,var(--warning)_40%,var(--foreground))] dark:hover:text-[color-mix(in_oklab,var(--warning)_65%,var(--foreground))]",
        )}
        disabled={!state.canReconnect}
        onClick={() => void reconnectConnection(connectionId, qc)}
      >
        <RotateCw />
        Reconnect
      </Button>
    </div>
  );
}
