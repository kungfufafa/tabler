import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Lock, ShieldCheck, SquareTerminal, TriangleAlert } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { effectiveLevel, safeModeText } from "@/features/safety/levels";
import {
  ipc,
  toIpcError,
  type ConnectionProfile,
  type ConnectionsFilePreview,
  type SkippedConnection,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { DriverBadge, GroupBreadcrumb, TagPill } from "./badges";
import { groupKeys } from "./groups";
import { connectionDetail } from "./schema";
import { profileKeys } from "./useConnections";

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/**
 * What an imported connection does on its own: its safe-mode level, Read-only, and the startup
 * SQL it runs on every connect (shown in full with a warning: a file from someone else could run
 * anything there).
 */
function ImportedBehaviour({ profile }: { profile: ConnectionProfile }) {
  const { options } = profile;
  const startupSql = options.startupSql?.trim() ?? "";
  return (
    <>
      <div className="text-muted-foreground mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px]">
        <span className="inline-flex items-center gap-1">
          <ShieldCheck className="size-3" aria-hidden />
          {safeModeText(effectiveLevel(options.safeMode))}
        </span>
        {options.readOnly && (
          <span className="inline-flex items-center gap-1">
            <Lock className="size-3" aria-hidden />
            Read-only
          </span>
        )}
        {startupSql && (
          <span className="text-warning inline-flex items-center gap-1 font-medium">
            <TriangleAlert className="size-3" aria-hidden />
            Runs startup SQL on connect
          </span>
        )}
      </div>
      {startupSql && (
        <pre
          aria-label={`Startup SQL of ${profile.name}`}
          className="border-warning/40 bg-warning/5 mt-1 max-h-24 overflow-auto rounded border px-1.5 py-1 font-mono text-[11px] whitespace-pre-wrap"
        >
          {startupSql}
        </pre>
      )}
    </>
  );
}
/**
 * A connection that gets its password from a shell command ("Command Line (IAM
 * authentication)"): the command runs on every connect, so it is shown in full and comes in only
 * when the user ticks "Keep this password command" (off by default; without it the password is
 * asked on every connect).
 */
function ImportedPasswordCommand({
  profile,
  keep,
  onKeepChange,
}: {
  profile: ConnectionProfile;
  keep: boolean;
  onKeepChange: (keep: boolean) => void;
}) {
  const command = profile.options.passwordCommand?.trim() ?? "";
  if (!command) return null;
  return (
    <div
      role="group"
      aria-label={`Password command of ${profile.name}`}
      className="border-warning/40 bg-warning/5 mt-1 rounded border px-1.5 py-1 text-[11px]"
    >
      <div className="text-warning flex items-center gap-1 font-medium">
        <SquareTerminal className="size-3 shrink-0" aria-hidden />
        Runs this shell command to get its password
      </div>
      <pre className="mt-0.5 max-h-24 overflow-auto font-mono whitespace-pre-wrap">{command}</pre>
      <label className="mt-1 flex items-center gap-1.5">
        <Checkbox checked={keep} onCheckedChange={(v) => onKeepChange(v === true)} />
        Keep this password command
      </label>
      {!keep && (
        <div className="text-muted-foreground mt-0.5">
          Without it, the password is asked on every connect.
        </div>
      )}
    </div>
  );
}

/**
 * The file's connections this version cannot read: a newer Tabler wrote them (a database it does
 * not support yet, or a setting it does not know). Listed so none goes missing unnoticed; they
 * are not imported.
 */
function SkippedConnections({ skipped }: { skipped: SkippedConnection[] }) {
  const count = skipped.length === 1 ? "1 connection needs" : `${skipped.length} connections need`;
  return (
    <div
      role="group"
      aria-label="Connections that cannot be imported"
      className="border-warning/40 bg-warning/5 rounded-md border px-2.5 py-2 text-[11px]"
    >
      <div className="text-warning flex items-center gap-1 font-medium">
        <TriangleAlert className="size-3 shrink-0" aria-hidden />
        {count} a newer version of Tabler and will not be imported
      </div>
      <ul className="mt-1 flex max-h-28 flex-col gap-0.5 overflow-y-auto">
        {skipped.map((s, i) => (
          <li key={i}>
            <span className="font-semibold">{s.name.trim() || "Unnamed connection"}</span>{" "}
            <span className="text-muted-foreground">{s.reason}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

const fileName = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;

type Phase =
  | { kind: "loading" }
  | { kind: "password"; error: string | null }
  | { kind: "preview"; preview: ConnectionsFilePreview }
  | { kind: "failed"; message: string };

/**
 * "Import Connections…": reads the picked file, asks for its password when it is encrypted, then
 * lists its connections (all ticked; names already in use are pointed out; each one's group path,
 * safe-mode level, Read-only, startup SQL and password command shown) and imports the ticked ones
 * as new connections. Their passwords go to the Keychain; a password command only when its "Keep
 * this password command" box is ticked; their groups (nested, with their icons) merge into the
 * saved groups by path. Connections the file holds that this version cannot read are listed
 * apart and left out.
 */
export function ImportConnectionsDialog({
  path,
  onClose,
  onImported,
}: {
  path: string | null;
  onClose: () => void;
  onImported?: (profiles: ConnectionProfile[]) => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const [password, setPassword] = useState("");
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  /** Connections (ids as in the file) whose password command the user chose to keep. */
  const [keepCommands, setKeepCommands] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);

  const showPreview = (preview: ConnectionsFilePreview) => {
    setSelected(new Set(preview.connections.map((c) => c.profile.id)));
    setKeepCommands(new Set());
    setPhase({ kind: "preview", preview });
  };

  useEffect(() => {
    if (!path) return;
    let cancelled = false;
    setPhase({ kind: "loading" });
    setPassword("");
    setBusy(false);
    ipc.profiles.inspectConnectionsFile(path, null).then(
      (preview) => {
        if (cancelled) return;
        if (preview.needsPassword) setPhase({ kind: "password", error: null });
        else showPreview(preview);
      },
      (err: unknown) => {
        if (!cancelled) setPhase({ kind: "failed", message: toIpcError(err).message });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [path]);

  const unlock = async () => {
    if (!path) return;
    setBusy(true);
    try {
      const preview = await ipc.profiles.inspectConnectionsFile(path, password);
      if (preview.needsPassword) setPhase({ kind: "password", error: "Enter the file password" });
      else showPreview(preview);
    } catch (err) {
      setPhase({ kind: "password", error: toIpcError(err).message });
    } finally {
      setBusy(false);
    }
  };

  const runImport = async () => {
    if (!path || phase.kind !== "preview") return;
    const ids = phase.preview.connections
      .map((c) => c.profile.id)
      .filter((pid) => selected.has(pid));
    const keep = ids.filter((pid) => keepCommands.has(pid));
    setBusy(true);
    try {
      const imported = await ipc.profiles.importConnections(
        path,
        phase.preview.encrypted ? password : null,
        ids,
        keep,
      );
      void qc.invalidateQueries({ queryKey: profileKeys.all });
      void qc.invalidateQueries({ queryKey: groupKeys.all });
      toast.success(`Imported ${plural(imported.length, "connection")}`, {
        description: fileName(path),
      });
      onImported?.(imported);
      onClose();
    } catch (err) {
      toast.error("Could not import the connections", { description: toIpcError(err).message });
    } finally {
      setBusy(false);
    }
  };

  const items = phase.kind === "preview" ? phase.preview.connections : [];
  const skipped = phase.kind === "preview" ? phase.preview.skipped : [];
  const allChecked = items.length > 0 && items.every((c) => selected.has(c.profile.id));
  const someChecked = items.some((c) => selected.has(c.profile.id));
  const toggleIn =
    (set: typeof setSelected) =>
    (pid: string, on: boolean): void =>
      set((s) => {
        const next = new Set(s);
        if (on) next.add(pid);
        else next.delete(pid);
        return next;
      });
  const toggle = toggleIn(setSelected);
  const toggleKeep = toggleIn(setKeepCommands);

  return (
    <Dialog open={path !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Import Connections</DialogTitle>
          <DialogDescription className="truncate" title={path ?? undefined}>
            {path ? fileName(path) : null}
          </DialogDescription>
        </DialogHeader>

        {phase.kind === "loading" && (
          <div className="text-muted-foreground flex items-center gap-2 py-6 text-xs">
            <Loader2 className="size-4 animate-spin" /> Reading the file…
          </div>
        )}

        {phase.kind === "failed" && (
          <p role="alert" className="text-error flex items-start gap-2 py-2 text-xs">
            <TriangleAlert className="mt-px size-4 shrink-0" />
            {phase.message}
          </p>
        )}

        {phase.kind === "password" && (
          <form
            id={`${id}-unlock`}
            className="flex flex-col gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              void unlock();
            }}
          >
            <label htmlFor={`${id}-password`} className="text-xs">
              This file is encrypted. Enter its password.
            </label>
            <Input
              id={`${id}-password`}
              type="password"
              aria-label="File password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-invalid={phase.error ? true : undefined}
              autoComplete="off"
              className="font-mono"
              autoFocus
            />
            {phase.error && (
              <p role="alert" className="text-error text-[11px]">
                {phase.error}
              </p>
            )}
          </form>
        )}

        {phase.kind === "preview" &&
          (items.length === 0 ? (
            skipped.length === 0 && (
              <p className="text-muted-foreground py-4 text-xs">The file has no connections.</p>
            )
          ) : (
            <div className="flex min-h-0 flex-col gap-2">
              <label className="text-muted-foreground flex items-center gap-2 px-1 text-xs">
                <Checkbox
                  aria-label="Select all"
                  checked={allChecked ? true : someChecked ? "indeterminate" : false}
                  onCheckedChange={(v) =>
                    setSelected(v === true ? new Set(items.map((c) => c.profile.id)) : new Set())
                  }
                />
                {plural(selected.size, "connection")} selected
              </label>
              <ul
                aria-label="Connections in the file"
                className="border-border min-h-0 overflow-y-auto rounded-md border"
              >
                {items.map((c) => (
                  <li
                    key={c.profile.id}
                    className="border-border flex items-center gap-2.5 border-b px-2.5 py-2 last:border-b-0"
                  >
                    <Checkbox
                      aria-label={`Import ${c.profile.name}`}
                      checked={selected.has(c.profile.id)}
                      onCheckedChange={(v) => toggle(c.profile.id, v === true)}
                    />
                    <DriverBadge
                      driver={c.profile.driver}
                      mysqlVersion={c.profile.options.mysqlVersion}
                      color={c.profile.color}
                    />
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 items-center gap-1.5">
                        <span className="truncate text-[13px] font-semibold">{c.profile.name}</span>
                        {c.profile.options.tag && <TagPill tag={c.profile.options.tag} />}
                        {(c.hasPassword || c.hasSshSecrets) && (
                          <KeyRound
                            className="text-muted-foreground size-3 shrink-0"
                            aria-label="Includes passwords"
                            role="img"
                          />
                        )}
                      </div>
                      <div className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-[11px]">
                        {c.profile.group && (
                          <GroupBreadcrumb
                            path={c.profile.group}
                            className="max-w-[55%] shrink-0"
                          />
                        )}
                        <span className="truncate font-mono">{connectionDetail(c.profile)}</span>
                      </div>
                      <ImportedBehaviour profile={c.profile} />
                      <ImportedPasswordCommand
                        profile={c.profile}
                        keep={keepCommands.has(c.profile.id)}
                        onKeepChange={(on) => toggleKeep(c.profile.id, on)}
                      />
                      {c.duplicateName && (
                        <div className={cn("text-warning mt-0.5 text-[11px]")}>
                          A connection named “{c.profile.name}” already exists; this one is added
                          next to it.
                        </div>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        {skipped.length > 0 && <SkippedConnections skipped={skipped} />}

        <DialogFooter>
          <Button type="button" variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          {phase.kind === "password" ? (
            <Button type="submit" size="sm" form={`${id}-unlock`} disabled={busy}>
              {busy ? <Loader2 className="animate-spin" /> : null}
              Continue
            </Button>
          ) : (
            <Button
              type="button"
              size="sm"
              onClick={() => void runImport()}
              disabled={busy || phase.kind !== "preview" || selected.size === 0}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}
              Import
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
