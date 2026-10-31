import { useState } from "react";
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
import { Textarea } from "@/components/ui/textarea";

/**
 * The object menu's questions (an index name and body) and confirmations (the
 * command a destructive item runs), asked from a menu item, which has no place in the tree of
 * its own: each opens in a root of its own and resolves with the answer, or null when declined.
 */
export interface Question {
  title: string;
  description?: string;
  /** A text field and what it starts with; none: a confirmation. */
  input?: { label: string; initial?: string; multiline?: boolean; placeholder?: string };
  /** The command that will run, shown as is. */
  command?: string;
  confirmLabel: string;
  destructive?: boolean;
}

export function QuestionDialog({
  q,
  done,
}: {
  q: Question;
  done: (answer: string | null) => void;
}) {
  const [open, setOpen] = useState(true);
  const [text, setText] = useState(q.input?.initial ?? "");
  const finish = (answer: string | null) => {
    setOpen(false);
    done(answer);
  };
  const ok = q.input === undefined || text.trim() !== "";
  return (
    <Dialog open={open} onOpenChange={(o) => !o && finish(null)}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{q.title}</DialogTitle>
          {q.description && <DialogDescription>{q.description}</DialogDescription>}
        </DialogHeader>
        {q.input &&
          (q.input.multiline ? (
            <Textarea
              aria-label={q.input.label}
              value={text}
              placeholder={q.input.placeholder}
              onChange={(e) => setText(e.target.value)}
              className="min-h-32 font-mono text-xs"
              spellCheck={false}
              autoFocus
            />
          ) : (
            <Input
              aria-label={q.input.label}
              value={text}
              placeholder={q.input.placeholder}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && ok) finish(text.trim());
              }}
              className="font-mono"
              spellCheck={false}
              autoFocus
            />
          ))}
        {q.command && (
          <pre
            data-testid="es-command"
            className="bg-muted max-h-48 overflow-auto rounded-md p-2 font-mono text-[11px] break-all whitespace-pre-wrap"
          >
            {q.command}
          </pre>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => finish(null)}>
            Cancel
          </Button>
          <Button
            variant={q.destructive ? "destructive" : "default"}
            disabled={!ok}
            onClick={() => finish(q.input ? text.trim() : "")}
          >
            {q.confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
