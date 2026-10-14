import { KeyRound } from "lucide-react";
import { useEffect, useId, useState } from "react";
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
import type { SshPromptRequest } from "@/lib/ipc";
import { listenEvent, type Unlisten } from "@/lib/ipc/events";
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
  const verb = request.purpose === "test" ? "Test" : "Connect";
  const title = request.name.trim() || "SSH authentication";
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
          <span className="text-foreground font-mono">{server(request)}</span> asks for more
          information
          {request.label ? (
            <>
              {" "}
              to open <span className="text-foreground">{request.label}</span>
            </>
          ) : null}
          .
        </AlertDialogDescription>
      </AlertDialogHeader>
      {request.instructions.trim() !== "" && (
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

/**
 * Listens for `ssh:prompt` and shows the questions one at a time. Mount once near the app root;
 * extra instances stay inert.
 */
export function SshPromptHost() {
  const owner = useSingleHost(SSH_PROMPT_HOST);
  const request = useSshPromptStore((s) => s.queue[0] ?? null);
  useEffect(() => {
    if (!owner) return;
    let gone = false;
    let unlisten: Unlisten[] = [];
    void Promise.all([
      listenEvent("ssh:prompt", enqueueSshPrompt),
      listenEvent("ssh:prompt-done", (done) => dropSshPrompt(done.requestId)),
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
