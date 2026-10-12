import { LoaderCircle, Unplug } from "lucide-react";
import { Fragment } from "react";
import { useProfiles } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import { useReconnectInFlight } from "@/hooks/useConnectionEvents";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";
import { useActiveTab } from "@/stores/tabs";
import { TAG_STYLE, tagColor } from "./connectionLabels";
import { pillModel, pillText, type SegmentRole } from "./statusPillModel";
import { ToolbarTip } from "./ToolbarButton";

/** Narrow pill: the middle parts truncate first, the server and the visible object last. */
const SHRINK: Record<SegmentRole, string> = {
  state: "shrink-0",
  server: "shrink",
  ssh: "shrink-[6]",
  name: "shrink-[8]",
  database: "shrink-[8]",
  schema: "shrink-[8]",
  object: "shrink-[2]",
};

const PILL =
  "flex h-6.5 min-w-70 max-w-160 flex-[0_1_640px] items-center justify-center gap-1.5 overflow-hidden rounded-md border px-2 font-mono text-[11.5px] leading-none outline-none focus-visible:ring-2 focus-visible:ring-ring/50";

/**
 * The toolbar's centred status pill: the active connection (tag, server, SSH, name, database) and
 * what the visible tab shows, tinted with the connection colour. Click → switch connection (⇧⌘K).
 */
export function StatusPill() {
  const active = useActiveConnection();
  const tab = useActiveTab();
  const profiles = useProfiles();
  const busy = useReconnectInFlight(active?.info.connectionId ?? null);

  if (!active) {
    return (
      <ToolbarTip label="Switch connection" action="palette.connections">
        <button
          type="button"
          data-slot="status-pill"
          aria-label="No connection (switch connection)"
          onClick={() => void actions.run("palette.connections")}
          className={cn(PILL, "border-border bg-surface text-muted-foreground font-sans text-xs")}
        >
          No connection
        </button>
      </ToolbarTip>
    );
  }

  const tag = profiles.data?.find((p) => p.id === active.info.profileId)?.options.tag ?? null;
  const model = pillModel(active, tab, tag, busy);
  const tint = model.problem ? "var(--warning)" : accentColor(active.info.color);
  const text = pillText(model);
  const StateIcon = model.problem === "reconnecting" ? LoaderCircle : Unplug;
  return (
    <ToolbarTip label="Switch connection" action="palette.connections">
      <button
        type="button"
        data-slot="status-pill"
        data-state={model.problem ?? "connected"}
        aria-label={`${text} (switch connection)`}
        onClick={() => void actions.run("palette.connections")}
        className={cn(
          PILL,
          "transition-[filter] hover:brightness-[0.97] dark:hover:brightness-110",
        )}
        style={{
          backgroundColor: `color-mix(in oklab, ${tint} 13%, var(--surface))`,
          borderColor: `color-mix(in oklab, ${tint} 35%, transparent)`,
          color: `color-mix(in oklab, ${tint} 50%, var(--foreground))`,
        }}
      >
        {model.tag && (
          <span
            data-slot="status-pill-tag"
            className="shrink-0 rounded-[3px] px-1 py-0.5 font-sans text-[9.5px] leading-none font-semibold tracking-wide text-white"
            style={{ backgroundColor: tagColor(model.tag) }}
          >
            {TAG_STYLE[model.tag].label}
          </span>
        )}
        {model.problem && (
          <StateIcon
            aria-hidden
            className={cn("size-3 shrink-0", model.problem === "reconnecting" && "animate-spin")}
          />
        )}
        <span className="flex min-w-0 items-center" aria-hidden>
          {model.segments.map((s, i) => (
            <Fragment key={`${s.role}:${i}`}>
              {i > 0 && <span className="shrink-0 px-1 opacity-50">:</span>}
              <span
                data-role={s.role}
                className={cn(
                  "min-w-0 truncate",
                  SHRINK[s.role],
                  (s.role === "object" || s.role === "state") && "font-semibold",
                )}
              >
                {s.text}
              </span>
            </Fragment>
          ))}
        </span>
      </button>
    </ToolbarTip>
  );
}
