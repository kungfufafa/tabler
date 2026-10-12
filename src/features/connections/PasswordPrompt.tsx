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
      <DialogContent className="sm:max-w-sm" showCloseButton={false}>
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (request) onAnswer(request.id, value);
          }}
        >
          <DialogHeader className="flex-row items-start gap-3">
            <span className="bg-brand-soft text-brand flex size-9 shrink-0 items-center justify-center rounded-lg">
              <KeyRound className="size-4.5" />
            </span>
            <div className="flex min-w-0 flex-col gap-1">
              <DialogTitle className="truncate">{shown?.title}</DialogTitle>
              {shown?.description ? (
                <DialogDescription className="text-xs">{shown.description}</DialogDescription>
              ) : null}
            </div>
          </DialogHeader>
          <div className="flex flex-col gap-1">
            <Input
              id={`${id}-password`}
              type="password"
              aria-label="Password"
              aria-invalid={shown?.error ? true : undefined}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              autoComplete="off"
              className="font-mono"
              autoFocus
            />
            {shown?.error ? (
              <p role="alert" className="text-error text-[11px]">
                {shown.error}
              </p>
            ) : null}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
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
