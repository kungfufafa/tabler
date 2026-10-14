import { KeyRound } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { hasSingleHost, useSingleHost } from "@/features/export/singleHost";
import { ipc, isMock, toIpcError, type SqliteSshEvent, type SshPromptRequest } from "@/lib/ipc";
import { listenEvent, listenWindowEvent, type Unlisten } from "@/lib/ipc/events";
import {
  answerSshPrompt,
  cancelSshPrompts,
  dropSshPrompt,
  enqueueSshPrompt,
  promptLabel,
  SSH_PROMPT_HOST,
  useSshPromptStore,
} from "./sshPromptStore";

const server = (r: SshPromptRequest) => `${r.user}@${r.host}${r.port === 22 ? "" : `:${r.port}`}`;

/** One round of questions: the server's instructions, one field per question. */
function Questions({
  request,
  onAnswer,
}: {
  request: SshPromptRequest;
  onAnswer: (requestId: string, answers: string[] | null) => void;
}) {
  const uid = useId();
  const [answers, setAnswers] = useState(() => request.prompts.map(() => ""));
  const confirm = request.kind === "confirm";
  const verb = confirm ? "Continue" : request.purpose === "test" ? "Test" : "Connect";
  const title = request.name.trim() || "SSH authentication";
  const opening = request.label ? (
    <>
      {" "}
      to open <span className="text-foreground">{request.label}</span>
    </>
  ) : null;
  return (
    <form
      className="contents"
      onSubmit={(e) => {
        e.preventDefault();
        onAnswer(request.requestId, answers);
      }}
    >
      <AlertDialogHeader>
        <AlertDialogTitle className="flex items-center gap-2">
          <KeyRound className="text-brand size-4 shrink-0" />
          {title}
        </AlertDialogTitle>
        <AlertDialogDescription className="text-xs">
          {confirm ? (
            request.instructions.trim()
          ) : request.kind === "password" ? (
            <>
              Enter the SSH password of{" "}
              <span className="text-foreground font-mono">{server(request)}</span>
              {opening}. It is not stored.
            </>
          ) : (
            <>
              <span className="text-foreground font-mono">{server(request)}</span> asks for more
              information
              {opening}.
            </>
          )}
        </AlertDialogDescription>
      </AlertDialogHeader>
      {!confirm && request.instructions.trim() !== "" && (
        <p
          data-testid="ssh-prompt-instructions"
          className="bg-surface-2 border-border text-foreground rounded-md border px-3 py-2 text-xs wrap-anywhere whitespace-pre-wrap"
        >
          {request.instructions.trim()}
        </p>
      )}
      <div className="flex flex-col gap-2.5">
        {request.prompts.map((p, i) => (
          <div key={i} className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-${i}`} className="text-xs font-normal">
              {promptLabel(p.prompt)}
            </Label>
            <Input
              id={`${uid}-${i}`}
              type={p.echo ? "text" : "password"}
              value={answers[i] ?? ""}
              onChange={(e) =>
                setAnswers((prev) => prev.map((a, j) => (j === i ? e.target.value : a)))
              }
              autoFocus={i === 0}
              autoComplete={p.echo ? "off" : "one-time-code"}
              spellCheck={false}
              className="font-mono"
            />
          </div>
        ))}
      </div>
      <AlertDialogFooter className="py-3">
        <AlertDialogCancel size="sm" type="button">
          Cancel
        </AlertDialogCancel>
        <Button type="submit" size="sm">
          {verb}
        </Button>
      </AlertDialogFooter>
    </form>
  );
}

/**
 * The SSH server's keyboard-interactive (2FA) questions (verification code, PIN…) during a
 * connect or test. Cancel (or Esc) gives the connect up.
 */
export function SshPrompt({
  request,
  onAnswer,
}: {
  request: SshPromptRequest | null;
  onAnswer: (requestId: string, answers: string[] | null) => void;
}) {
  // Keep showing the last round while the dialog animates out.
  const [last, setLast] = useState(request);
  if (request && request !== last) setLast(request);
  const shown = request ?? last;
  return (
    <AlertDialog
      open={request !== null}
      onOpenChange={(open) => {
        if (!open && request) onAnswer(request.requestId, null);
      }}
    >
      <AlertDialogContent className="grid-cols-1 gap-3 data-[size=default]:sm:max-w-md">
        {shown && <Questions key={shown.requestId} request={shown} onAnswer={onAnswer} />}
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Shows a file in Finder (no-op in the browser mock). */
async function reveal(path: string): Promise<void> {
  if (isMock) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/**
 * SQLite over SSH upload results: a quiet note after an upload, a warning with "Upload Anyway"
 * when the file on the server changed meanwhile, the kept copy's location after a failed last
 * upload.
 */
function showSqliteSshStatus(e: SqliteSshEvent): void {
  switch (e.kind) {
    case "uploaded":
      toast.success(`Uploaded changes of “${e.name}”`, { description: e.remote, duration: 2500 });
      break;
    case "conflict":
      toast.warning(`“${e.name}” changed on the SSH server`, {
        description: `${e.message}. Uploading replaces the server's version (${e.remote}).`,
        duration: 20_000,
        action: {
          label: "Upload Anyway",
          onClick: () =>
            void ipc.sqliteSsh.upload(e.connectionId, true).then(
              (done) => {
                if (done.uploaded) toast.success(`Uploaded changes of “${e.name}”`);
              },
              (err: unknown) =>
                toast.error("Could not upload the changes", {
                  description: toIpcError(err).message,
                }),
            ),
        },
      });
      break;
    case "error":
      toast.error(`Could not upload the changes of “${e.name}”`, { description: e.message });
      break;
    case "keptLocal":
      toast.warning(e.message, {
        description: e.localPath ?? e.remote,
        duration: Infinity,
        ...(e.localPath
          ? { action: { label: "Show in Finder", onClick: () => void reveal(e.localPath!) } }
          : {}),
      });
      break;
  }
}

/**
 * Listens for `ssh:prompt` and shows the questions one at a time. Mount once near the app root;
 * extra instances stay inert. The backend sends a question to the window that started the connect
 * (or the focused one) and upload results to the window that owns the connection, so they are
 * heard on this window only; "done" goes to every window (only the one showing it has it).
 */
export function SshPromptHost() {
  const owner = useSingleHost(SSH_PROMPT_HOST);
  const request = useSshPromptStore((s) => s.queue[0] ?? null);
  useEffect(() => {
    if (!owner) return;
    let gone = false;
    let unlisten: Unlisten[] = [];
    void Promise.all([
      listenWindowEvent("ssh:prompt", enqueueSshPrompt),
      listenEvent("ssh:prompt-done", (done) => dropSshPrompt(done.requestId)),
      listenWindowEvent("sqlite-ssh:status", showSqliteSshStatus),
    ]).then((fns) => {
      if (gone) fns.forEach((f) => f());
      else unlisten = fns;
    });
    return () => {
      gone = true;
      unlisten.forEach((f) => f());
    };
  }, [owner]);
  useEffect(
    () => () => {
      // Nobody is left to answer: give the waiting connects up instead of leaving them hanging.
      queueMicrotask(() => {
        if (!hasSingleHost(SSH_PROMPT_HOST)) cancelSshPrompts();
      });
    },
    [],
  );
  if (!owner) return null;
  return <SshPrompt request={request} onAnswer={answerSshPrompt} />;
}
