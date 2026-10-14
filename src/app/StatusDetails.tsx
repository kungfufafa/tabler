import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { TagPill } from "@/features/connections/badges";
import { CustomColorSwatch } from "@/features/connections/CustomColorSwatch";
import { profileKeys } from "@/features/connections/useConnections";
import { openConnectionDialog } from "@/features/connections/store";
import { actions } from "@/features/palette/actions";
import { toIpcError, type ConnectionProfile, type SessionDetails } from "@/lib/ipc";
import { ACCENT_PRESETS, accentColor, type AccentPreset } from "@/lib/theme";
import { cn } from "@/lib/utils";
import type { OpenConnection } from "@/stores/connections";
import { connectedSince, sinceLabel } from "./connectedSince";
import { databaseLabel } from "./connectionLabels";
import { hostLabel, recolorConnection, tlsLabel, tunnelLabel } from "./statusDetailsModel";

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate font-mono text-[11px]" title={String(children ?? "")}>
        {children}
      </dd>
    </>
  );
}

/** The colour row: TablePlus's status colours (the connection form's presets, and Custom…). */
function ColorRow({ conn }: { conn: OpenConnection }) {
  const qc = useQueryClient();
  const [saving, setSaving] = useState(false);
  const current = conn.info.color ?? "blue";
  // One save at a time. What the user picks while one runs (the custom picker fires on every step
  // of a drag) is kept, the latest only, and saved when the running one ends: the colour the drag
  // was released on is the one that stays.
  const busy = useRef(false);
  const wanted = useRef<string | null>(null);
  const takeWanted = () => {
    const color = wanted.current;
    wanted.current = null;
    return color;
  };
  const choose = async (color: AccentPreset | string) => {
    if (busy.current) {
      wanted.current = color;
      return;
    }
    if (color === current) return;
    busy.current = true;
    setSaving(true);
    try {
      let next: string | null = color;
      while (next !== null) {
        const saved: string = next;
        await recolorConnection(conn.info.profileId, saved);
        const later = takeWanted();
        next = later !== null && later !== saved ? later : null;
      }
      void qc.invalidateQueries({ queryKey: profileKeys.all });
    } catch (err) {
      toast.error("Could not change the colour", { description: toIpcError(err).message });
    } finally {
      wanted.current = null;
      busy.current = false;
      setSaving(false);
    }
  };
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-muted-foreground text-xs">Colour</span>
      <div role="radiogroup" aria-label="Connection colour" className="flex items-center gap-1.5">
        {ACCENT_PRESETS.map((c) => (
          <button
            key={c}
            type="button"
            role="radio"
            aria-checked={current === c}
            aria-label={c}
            title={c}
            disabled={saving}
            onClick={() => void choose(c)}
            className={cn(
              "ring-offset-popover size-4 rounded-full ring-offset-2 transition-transform outline-none",
              "focus-visible:ring-ring focus-visible:ring-2 disabled:opacity-60",
              current === c ? "ring-foreground/60 scale-110 ring-2" : "hover:scale-110",
            )}
            style={{ background: accentColor(c) }}
          />
        ))}
        <CustomColorSwatch
          value={current}
          disabled={saving}
          onChange={(hex) => void choose(hex)}
          className="ring-offset-popover"
        />
      </div>
    </div>
  );
}

/**
 * The status bar's details (click the pill): server, user, host, database, schema, encoding,
 * TLS, tunnel and session start, then the connection colour and the switch / edit actions.
 */
export function StatusDetails({
  conn,
  profile,
  details,
  onClose,
}: {
  conn: OpenConnection;
  profile: ConnectionProfile | null;
  details: SessionDetails | null;
  onClose: () => void;
}) {
  const { info } = conn;
  const at = connectedSince(info.connectionId);
  const tag = profile?.options.tag ?? null;
  return (
    <div data-slot="status-details" className="flex flex-col gap-3 text-xs">
      <div className="flex min-w-0 items-center gap-2">
        <span
          aria-hidden
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: accentColor(info.color) }}
        />
        <span className="min-w-0 truncate text-[13px] font-semibold">{info.name}</span>
        {tag && <TagPill tag={tag} />}
      </div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
        <Row label="Server">{info.serverVersion || "—"}</Row>
        <Row label="User">{info.currentUser ?? "—"}</Row>
        <Row label={info.driver === "sqlite" ? "File" : "Host"}>{hostLabel(profile) ?? "—"}</Row>
        {info.driver !== "sqlite" && <Row label="Database">{databaseLabel(info) || "—"}</Row>}
        <Row label="Schema">{conn.currentSchema ?? "—"}</Row>
        <Row label="Encoding">{details?.encoding ?? "…"}</Row>
        <Row label="TLS">{tlsLabel(details, info.driver)}</Row>
        <Row label="Tunnel">{tunnelLabel(conn, profile)}</Row>
        <Row label="Connected">{at === null ? "—" : sinceLabel(at)}</Row>
      </dl>
      <ColorRow conn={conn} />
      <div className="border-border flex items-center justify-end gap-1.5 border-t pt-2.5">
        <Button
          variant="ghost"
          size="xs"
          disabled={!profile}
          onClick={() => {
            onClose();
            if (profile) openConnectionDialog(profile);
          }}
        >
          Edit Connection…
        </Button>
        <Button
          variant="outline"
          size="xs"
          onClick={() => {
            onClose();
            void actions.run("palette.connections");
          }}
        >
          Switch Connection…
        </Button>
      </div>
    </div>
  );
}
