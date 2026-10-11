import { Check, Copy, ShieldAlert, ShieldQuestion } from "lucide-react";
import { useEffect, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { hasSingleHost, useSingleHost } from "@/features/export/singleHost";
import { cn } from "@/lib/utils";
import {
  abandonHostKeyPrompts,
  answerHostKeyPrompt,
  HOST_KEY_PROMPT_HOST,
  keyTypeLabel,
  useHostKeyPromptStore,
  type HostKeyDecision,
  type HostKeyPromptRequest,
} from "./useConnections";

/** `host:port`, with OpenSSH's `[host]:port` form for `ssh-keygen -R` when the port is not 22. */
const hostPort = (r: HostKeyPromptRequest) => `${r.host}:${r.port}`;
const knownHostsName = (r: HostKeyPromptRequest) =>
  r.port === 22 ? r.host : `"[${r.host}]:${r.port}"`;

function Fingerprint({
  request,
  tone,
}: {
  request: HostKeyPromptRequest;
  tone: "neutral" | "danger";
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <div
      className={cn(
        "rounded-md border px-3 py-2",
        tone === "danger" ? "border-error/40 bg-error/5" : "border-border bg-surface-2",
      )}
    >
      <div className="text-muted-foreground text-[11px]">
        {keyTypeLabel(request.keyType)} key fingerprint
        {tone === "danger" ? " offered by the server" : ""}
      </div>
      <div className="mt-0.5 flex items-start gap-1.5">
        <code
          data-testid="host-key-fingerprint"
          className="text-foreground min-w-0 flex-1 cursor-text font-mono text-xs leading-5 break-all select-text"
        >
          {request.fingerprint}
        </code>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label={copied ? "Copied" : "Copy fingerprint"}
          title="Copy fingerprint"
          onClick={() => void writeClipboard(request.fingerprint).then(() => setCopied(true))}
        >
          {copied ? <Check className="text-success" /> : <Copy />}
        </Button>
      </div>
    </div>
  );
}

function UnknownKey({
  request,
  onAnswer,
}: {
  request: HostKeyPromptRequest;
  onAnswer: (id: number, decision: HostKeyDecision | null) => void;
}) {
  const [persist, setPersist] = useState(false);
  const verb = request.purpose === "test" ? "Test" : "Connect";
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle className="flex items-center gap-2">
          <ShieldQuestion className="text-warning size-4 shrink-0" />
          Unknown SSH host
        </AlertDialogTitle>
        <AlertDialogDescription className="text-xs">
          The authenticity of host{" "}
          <span className="text-foreground font-mono">{hostPort(request)}</span> can't be
          established
          {request.label ? (
            <>
              {" "}
              (<span className="text-foreground">{request.label}</span>)
            </>
          ) : null}
          . Make sure the fingerprint matches the server's before you trust it.
        </AlertDialogDescription>
      </AlertDialogHeader>
      <Fingerprint request={request} tone="neutral" />
      <label className="flex w-fit cursor-default items-center gap-2 text-xs">
        <Checkbox checked={persist} onCheckedChange={(v) => setPersist(v === true)} />
        Also add to <span className="font-mono">~/.ssh/known_hosts</span>
      </label>
      <AlertDialogFooter className="py-3">
        <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
        <AlertDialogAction size="sm" onClick={() => onAnswer(request.id, { persist })}>
          Trust &amp; {verb}
        </AlertDialogAction>
      </AlertDialogFooter>
    </>
  );
}

function ChangedKey({ request }: { request: HostKeyPromptRequest }) {
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle className="text-error flex items-center gap-2">
          <ShieldAlert className="size-4 shrink-0" />
          SSH host key has changed
        </AlertDialogTitle>
        <AlertDialogDescription className="text-xs">
          The key presented by{" "}
          <span className="text-foreground font-mono">{hostPort(request)}</span> does not match the
          one trusted before. Someone could be eavesdropping on you right now (a man-in-the-middle
          attack), or the server's host key was replaced.
          {request.label ? (
            <>
              {" "}
              <span className="text-foreground">{request.label}</span> was not opened.
            </>
          ) : null}
        </AlertDialogDescription>
      </AlertDialogHeader>
      <Fingerprint request={request} tone="danger" />
      <div className="text-muted-foreground flex min-w-0 flex-col gap-1.5 text-xs">
        <p>
          If the key was rotated on purpose, confirm the new fingerprint with the server's
          administrator, remove the old entry and connect again:
        </p>
        {/* Long bastion names wrap instead of widening the dialog. */}
        <code className="bg-surface-2 border-border text-foreground rounded-md border px-2 py-1 font-mono text-[11px] leading-4 wrap-anywhere whitespace-pre-wrap select-text">
          ssh-keygen -R {knownHostsName(request)}
        </code>
      </div>
      <AlertDialogFooter className="py-3">
        <AlertDialogCancel size="sm">Close</AlertDialogCancel>
      </AlertDialogFooter>
    </>
  );
}

/**
 * Asks whether to trust an unknown SSH host key (fingerprint, copy, "add to known_hosts"), or warns
 * about a changed one (no way to trust it from here).
 */
export function HostKeyPrompt({
  request,
  onAnswer,
}: {
  request: HostKeyPromptRequest | null;
  onAnswer: (id: number, decision: HostKeyDecision | null) => void;
}) {
  // Keep showing the last prompt while the dialog animates out.
  const [last, setLast] = useState(request);
  if (request && request !== last) setLast(request);
  const shown = request ?? last;
  return (
    <AlertDialog
      open={request !== null}
      onOpenChange={(open) => {
        if (!open && request) onAnswer(request.id, null);
      }}
    >
      <AlertDialogContent
        className={cn(
          // One track that never grows past the dialog's width, whatever its content.
          "grid-cols-1 gap-3 data-[size=default]:sm:max-w-md",
          shown?.changed && "ring-error/40",
        )}
      >
        {shown &&
          (shown.changed ? (
            <ChangedKey request={shown} />
          ) : (
            <UnknownKey key={shown.id} request={shown} onAnswer={onAnswer} />
          ))}
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * Renders the pending host-key prompt (requested by `withHostKeyTrust`). Mount once near the app
 * root; extra instances stay inert.
 */
export function HostKeyPromptHost() {
  const owner = useSingleHost(HOST_KEY_PROMPT_HOST);
  const request = useHostKeyPromptStore((s) => s.queue[0] ?? null);
  useEffect(
    () => () => {
      // Checked after the commit's other effects, so a host mounted in the same commit (a move to
      // another mount point) takes the queue over. Otherwise nobody could answer the pending
      // prompts: release their connects instead of leaving them hanging.
      queueMicrotask(() => {
        if (!hasSingleHost(HOST_KEY_PROMPT_HOST)) abandonHostKeyPrompts();
      });
    },
    [],
  );
  if (!owner) return null;
  return <HostKeyPrompt request={request} onAnswer={answerHostKeyPrompt} />;
}
