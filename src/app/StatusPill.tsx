import { LoaderCircle, Unplug } from "lucide-react";
import { useState, type CSSProperties } from "react";
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
  "focus-visible:ring-ring/50 flex h-8 min-w-48 max-w-160 items-center @max-3xl/toolbar:min-w-28 justify-center gap-2 overflow-hidden rounded-md border px-2 text-[12px] leading-4 outline-none transition-colors focus-visible:ring-2";

/** Without a connection: a plain surface. */
const NEUTRAL = "border-border bg-surface hover:bg-accent aria-expanded:bg-accent";

/**
 * Connected: tinted with the connection colour (`--pill-accent`), as TablePlus fills its pill with
 * it, the "which server am I on" cue at a glance. A tint rather than a solid fill keeps the muted
 * segments, the separators and the tag chip readable without recolouring each of them.
 */
const TINTED =
  "border-[color-mix(in_oklab,var(--pill-accent)_45%,var(--border))] bg-[color-mix(in_oklab,var(--pill-accent)_22%,var(--surface))] hover:bg-[color-mix(in_oklab,var(--pill-accent)_30%,var(--surface))] aria-expanded:bg-[color-mix(in_oklab,var(--pill-accent)_30%,var(--surface))]";

/** Lost / reconnecting: the warning colour, whatever the connection's own. */
const WARNING = "bg-warning/10 border-warning/40 hover:bg-warning/15 aria-expanded:bg-warning/15";

/**
 * The toolbar's centred status pill: the active connection (tag, server, SSH / TLS, name,
 * database) and what the visible tab shows, tinted with the connection colour. Click → the
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
          className={cn(PILL, NEUTRAL, "text-muted-foreground")}
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
            className={cn(PILL, model.problem ? WARNING : TINTED)}
            style={
              model.problem
                ? undefined
                : ({ "--pill-accent": accentColor(active.info.color) } as CSSProperties)
            }
          >
            {model.problem && <StateIcon aria-hidden className="size-4 shrink-0" />}
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
      <PopoverContent align="center" sideOffset={6} className="w-80 p-3">
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
