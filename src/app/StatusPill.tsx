import { LoaderCircle, Unplug } from "lucide-react";
import { useState } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useCustomTags } from "@/features/connections/tags";
import { useProfiles } from "@/features/connections/useConnections";
import { actions } from "@/features/palette/actions";
import { useReconnectInFlight } from "@/hooks/useConnectionEvents";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useActiveConnection } from "@/stores/connections";
import { useActiveTab } from "@/stores/tabs";
import { tagColor } from "./connectionLabels";
import { StatusDetails } from "./StatusDetails";
import { useSessionDetails } from "./statusDetailsModel";
import { pillModel, pillText, tagLabel, type SegmentRole } from "./statusPillModel";
import { ToolbarTip } from "./ToolbarButton";

/** Preserve the profile and visible object as long as possible at narrow widths. */
const SHRINK: Record<SegmentRole, string> = {
  state: "shrink-0",
  server: "shrink-[3]",
  transport: "shrink-[6]",
  name: "shrink",
  database: "shrink-[4]",
  schema: "shrink-[8]",
  object: "shrink",
};

const PILL =
  "border-border bg-surface hover:bg-accent aria-expanded:bg-accent focus-visible:ring-ring/60 flex h-8 min-w-48 max-w-160 items-center @max-3xl/toolbar:min-w-28 justify-center gap-2 overflow-hidden rounded-md border px-2 text-[12px] leading-4 outline-none transition-colors focus-visible:ring-2";

/**
 * The toolbar's centred status pill: the active connection (tag, server, SSH / TLS, name,
 * database) and what the visible tab shows, marked with the connection colour. Click → the
 * session's details with the connection colour (TablePlus: "click the status bar to see details
 * and recolour the connection"); without a connection → switch connection (⇧⌘K).
 */
export function StatusPill() {
  const active = useActiveConnection();
  const tab = useActiveTab();
  const profiles = useProfiles();
  const busy = useReconnectInFlight(active?.info.connectionId ?? null);
  // Custom tag colours (re-renders once they are loaded or edited).
  useCustomTags();
  const details = useSessionDetails(active);
  const [open, setOpen] = useState(false);

  if (!active) {
    return (
      <ToolbarTip label="Switch connection" action="palette.connections">
        <button
          type="button"
          data-slot="status-pill"
          aria-label="No connection (switch connection)"
          onClick={() => void actions.run("palette.connections")}
          className={cn(PILL, "text-muted-foreground")}
        >
          No connection
        </button>
      </ToolbarTip>
    );
  }

  const profile = profiles.data?.find((p) => p.id === active.info.profileId) ?? null;
  const tag = profile?.options.tag ?? null;
  const model = pillModel(active, tab, tag, busy, details?.tls ?? false);
  const text = pillText(model);
  const StateIcon = model.problem === "reconnecting" ? LoaderCircle : Unplug;
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <ToolbarTip label="Connection details">
        <PopoverTrigger asChild>
          <button
            type="button"
            data-slot="status-pill"
            data-state={model.problem ?? "connected"}
            aria-label={`${text} (connection details)`}
            className={PILL}
            style={
              model.problem
                ? {
                    backgroundColor: "color-mix(in oklab, var(--warning) 12%, var(--surface))",
                    borderColor: "color-mix(in oklab, var(--warning) 40%, var(--border))",
                  }
                : undefined
            }
          >
            {model.problem ? (
              <StateIcon aria-hidden className={cn("size-4 shrink-0")} />
            ) : (
              <span
                aria-hidden
                className="size-2 shrink-0 rounded-full"
                style={{ backgroundColor: accentColor(active.info.color) }}
              />
            )}
            {model.tag && (
              <span
                data-slot="status-pill-tag"
                className="text-foreground max-w-24 shrink-0 truncate rounded px-1 text-xs font-medium"
                style={{
                  backgroundColor: `color-mix(in oklab, ${tagColor(model.tag)} 18%, var(--surface))`,
                }}
              >
                {tagLabel(model.tag)}
              </span>
            )}
            <span className="flex min-w-0 items-center" aria-hidden>
              {model.segments.map((s, i) => (
                <span
                  key={`${s.role}:${i}`}
                  data-role={s.role}
                  className={cn(
                    "flex min-w-0 items-center",
                    SHRINK[s.role],
                    s.role === "schema" && "hidden @min-[920px]/toolbar:flex",
                  )}
                >
                  {i > 0 && <span className="text-muted-foreground shrink-0 px-0.5">·</span>}
                  <span
                    className={cn(
                      "min-w-0 truncate",
                      s.role === "object" || s.role === "state" || s.role === "name"
                        ? "text-foreground font-medium"
                        : "text-muted-foreground",
                      (s.role === "database" || s.role === "schema" || s.role === "object") &&
                        "font-mono",
                    )}
                  >
                    {s.text}
                  </span>
                </span>
              ))}
            </span>
          </button>
        </PopoverTrigger>
      </ToolbarTip>
      <PopoverContent align="center" sideOffset={6} className="w-84 p-3">
        <StatusDetails
          conn={active}
          profile={profile}
          details={details}
          onClose={() => setOpen(false)}
        />
      </PopoverContent>
    </Popover>
  );
}
