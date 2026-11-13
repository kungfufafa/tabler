import { LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { UpdateProgress } from "@/lib/ipc";
import { buildLabel, useAppInfo } from "@/stores/appInfo";
import { notesSince, progressLabel, progressPercent, publishedLabel } from "./labels";
import {
  cancelDownload,
  checkForUpdates,
  closeUpdateDialog,
  downloadUpdate,
  installUpdate,
  isBusy,
  useUpdaterStore,
  type AvailableUpdate,
  type UpdatePhase,
} from "./store";

function titleOf(phase: UpdatePhase): string {
  switch (phase.kind) {
    case "closed":
    case "checking":
      return "Checking for updates…";
    case "upToDate":
      return "Tabler is up to date";
    case "unsupported":
      return "Updates are not available";
    case "available":
    case "downloading":
      return `Tabler ${phase.update.version} is available`;
    case "ready":
    case "installing":
      return `Tabler ${phase.update.version} is ready to install`;
    case "error":
      return phase.update ? "The update did not finish" : "Could not check for updates";
  }
}

function Notes({ update }: { update: AvailableUpdate }) {
  const published = publishedLabel(update.date);
  // Only what is new to this install: the release lists older versions too.
  const notes = notesSince(update.notes, update.currentVersion);
  return (
    <div className="flex flex-col gap-1.5">
      {published && <p className="text-muted-foreground text-[11px]">Published {published}</p>}
      {notes && (
        <div
          className="selectable border-border bg-surface max-h-48 overflow-y-auto rounded-md border px-2.5 py-2 text-xs leading-5 whitespace-pre-wrap"
          aria-label="What changed"
        >
          {notes}
        </div>
      )}
    </div>
  );
}

function Progress({ progress }: { progress: UpdateProgress | null }) {
  const percent = progressPercent(progress);
  return (
    <div className="flex flex-col gap-1.5">
      <div
        role="progressbar"
        aria-label="Downloading the update"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent ?? undefined}
        className="bg-muted h-1.5 overflow-hidden rounded-full"
      >
        <div
          className={
            percent === null
              ? "bg-primary h-full w-1/3 animate-pulse rounded-full"
              : "bg-primary h-full rounded-full transition-[width] duration-150"
          }
          style={percent === null ? undefined : { width: `${percent}%` }}
        />
      </div>
      <p className="text-muted-foreground text-[11px] tabular-nums">
        {progressLabel(progress)}
        {percent !== null && ` · ${percent}%`}
      </p>
    </div>
  );
}

/**
 * Check for Updates…: what the check found, the new build's notes, the download with its
 * progress, and the restart that finishes the install. Outside clicks and Esc do nothing while a
 * download or an install runs (Cancel stops a download).
 */
export function UpdateDialog() {
  const phase = useUpdaterStore((s) => s.phase);
  const info = useAppInfo();
  const busy = isBusy(phase);
  const running = buildLabel(info);
  const channel = info?.channel === "nightly" ? "nightly" : "stable";

  return (
    <Dialog open={phase.kind !== "closed"} onOpenChange={(open) => !open && closeUpdateDialog()}>
      <DialogContent
        className="sm:max-w-md"
        showCloseButton={!busy}
        onInteractOutside={(e) => busy && e.preventDefault()}
        onEscapeKeyDown={(e) => busy && e.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle>{titleOf(phase)}</DialogTitle>
          <DialogDescription>
            {phase.kind === "checking" && "Asking the release page for the newest build."}
            {phase.kind === "upToDate" &&
              `${running || phase.version} is the newest build of the ${channel} channel.`}
            {phase.kind === "unsupported" && phase.reason}
            {(phase.kind === "available" || phase.kind === "downloading") &&
              `You have ${running || phase.update.currentVersion}.`}
            {phase.kind === "ready" &&
              "The update is downloaded and verified. Tabler restarts to finish installing it."}
            {phase.kind === "installing" && "Installing the update, then restarting…"}
            {phase.kind === "error" && phase.message}
          </DialogDescription>
        </DialogHeader>

        {phase.kind === "checking" && (
          <div className="text-muted-foreground flex items-center gap-2 text-xs">
            <LoaderCircle className="size-3.5 animate-spin" aria-hidden />
            Checking…
          </div>
        )}
        {(phase.kind === "available" || phase.kind === "ready") && <Notes update={phase.update} />}
        {phase.kind === "downloading" && <Progress progress={phase.progress} />}

        <DialogFooter>
          {(phase.kind === "upToDate" || phase.kind === "unsupported") && (
            <Button size="sm" onClick={closeUpdateDialog}>
              OK
            </Button>
          )}
          {phase.kind === "available" && (
            <>
              <Button variant="outline" size="sm" onClick={closeUpdateDialog}>
                Later
              </Button>
              <Button size="sm" onClick={() => void downloadUpdate()}>
                Download Update
              </Button>
            </>
          )}
          {phase.kind === "downloading" && (
            <Button variant="outline" size="sm" onClick={() => void cancelDownload()}>
              Cancel
            </Button>
          )}
          {(phase.kind === "ready" || phase.kind === "installing") && (
            <>
              <Button
                variant="outline"
                size="sm"
                disabled={phase.kind === "installing"}
                onClick={closeUpdateDialog}
              >
                Later
              </Button>
              <Button
                size="sm"
                disabled={phase.kind === "installing"}
                onClick={() => void installUpdate()}
              >
                Install and Restart
              </Button>
            </>
          )}
          {phase.kind === "error" && (
            <>
              <Button variant="outline" size="sm" onClick={closeUpdateDialog}>
                Close
              </Button>
              <Button
                size="sm"
                onClick={() => void (phase.update ? downloadUpdate() : checkForUpdates())}
              >
                Try Again
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
