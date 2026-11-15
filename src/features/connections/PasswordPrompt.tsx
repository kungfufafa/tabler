import { KeyRound } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { hasSingleHost, useSingleHost } from "@/features/export/singleHost";
import {
  answerPasswordPrompt,
  cancelPasswordPrompts,
  PASSWORD_PROMPT_HOST,
  usePasswordPromptStore,
  type PasswordPromptRequest,
} from "./passwordPromptStore";

/** One password field; ↩ confirms, Esc / Cancel resolves null. */
export function PasswordPrompt({
  request,
  onAnswer,
}: {
  request: PasswordPromptRequest | null;
  onAnswer: (id: number, password: string | null) => void;
}) {
  // Keep showing the last prompt while the dialog animates out.
  const [last, setLast] = useState(request);
  if (request && request !== last) setLast(request);
  const shown = request ?? last;
  const [value, setValue] = useState("");
  const id = useId();
  useEffect(() => setValue(""), [request?.id]);
  return (
    <Dialog
      open={request !== null}
      onOpenChange={(open) => {
        if (!open && request) onAnswer(request.id, null);
      }}
    >
      <DialogContent className="sm:max-w-md" showCloseButton={false}>
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (request) onAnswer(request.id, value);
          }}
        >
          <DialogHeader>
            <DialogTitle className="flex min-w-0 items-center gap-2 text-sm">
              <KeyRound aria-hidden className="text-brand size-4 shrink-0" />
              <span className="truncate">{shown?.title}</span>
            </DialogTitle>
            {shown?.description ? (
              <DialogDescription className="text-xs">{shown.description}</DialogDescription>
            ) : null}
          </DialogHeader>
          <div className="flex flex-col gap-1">
            <Input
              id={`${id}-password`}
              type="password"
              aria-label="Password"
              aria-invalid={shown?.error ? true : undefined}
              aria-describedby={shown?.error ? `${id}-error` : undefined}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              autoComplete="off"
              className="font-mono text-sm"
              autoFocus
            />
            {shown?.error ? (
              <p id={`${id}-error`} role="alert" className="text-error text-xs leading-4">
                {shown.error}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => request && onAnswer(request.id, null)}
            >
              Cancel
            </Button>
            <Button type="submit" size="sm">
              {shown?.confirmLabel ?? "OK"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Renders the pending password prompt (`requestPassword`). Mount once; extra instances stay inert. */
export function PasswordPromptHost() {
  const owner = useSingleHost(PASSWORD_PROMPT_HOST);
  const request = usePasswordPromptStore((s) => s.queue[0] ?? null);
  useEffect(
    () => () => {
      // A host mounted in the same commit takes the queue over; otherwise nobody could answer.
      queueMicrotask(() => {
        if (!hasSingleHost(PASSWORD_PROMPT_HOST)) cancelPasswordPrompts();
      });
    },
    [],
  );
  if (!owner) return null;
  return <PasswordPrompt request={request} onAnswer={answerPasswordPrompt} />;
}
