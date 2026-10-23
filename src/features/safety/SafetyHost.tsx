import { FingerprintPattern, Loader2, LockKeyhole, ShieldAlert } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
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
import { Input } from "@/components/ui/input";
import { TagPill } from "@/features/connections/badges";
import { hasSingleHost, useSingleHost } from "@/features/export/singleHost";
import { authenticateOwner } from "@/features/lock/biometric";
import { ipc, toIpcError } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { usePrefs } from "@/stores/prefs";
import { safeModeText } from "./levels";
import {
  abandonSafetyPrompts,
  answerSafetyPrompt,
  SAFETY_HOST,
  useSafetyPromptStore,
  type PasswordPrompt,
  type PromptTarget,
  type SafetyPrompt,
} from "./store";

/** Statements listed in a prompt; the rest are counted ("+N more"). */
export const MAX_LISTED_STATEMENTS = 50;
/** Longer statements are cut in the list (the full text is still what gets sent). */
const MAX_STATEMENT_CHARS = 2000;

function shorten(sql: string): string {
  const text = sql.trim();
  return text.length > MAX_STATEMENT_CHARS ? `${text.slice(0, MAX_STATEMENT_CHARS)}…` : text;
}

/** Connection name, environment tag and the safe-mode level asking. */
function ConnectionLine({ target }: { target: PromptTarget }) {
  return (
    <div
      data-slot="safety-connection"
      className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-1.5 text-xs"
    >
      <span className="text-foreground min-w-0 truncate font-medium">{target.name}</span>
      {target.tag && <TagPill tag={target.tag} />}
      <span aria-hidden>·</span>
      <span>{safeModeText(target.level)}</span>
    </div>
  );
}

function StatementList({ statements }: { statements: string[] }) {
  if (statements.length === 0) return null;
  const shown = statements.slice(0, MAX_LISTED_STATEMENTS);
  const more = statements.length - shown.length;
  return (
    <div
      role="list"
      aria-label="Queries"
      tabIndex={-1}
      className="border-border bg-surface-2 max-h-56 min-w-0 overflow-auto rounded-md border font-mono text-[11px] leading-4"
    >
      {shown.map((sql, i) => (
        <div
          // Statements are a fixed snapshot: their position is their identity.
          key={i}
          role="listitem"
          className="border-border text-foreground cursor-text border-b px-2 py-1.5 break-all whitespace-pre-wrap select-text last:border-b-0"
        >
          {shorten(sql)}
        </div>
      ))}
      {more > 0 && (
        <div role="listitem" className="text-muted-foreground px-2 py-1.5 font-sans">
          +{more} more
        </div>
      )}
    </div>
  );
}

function Header({ prompt }: { prompt: SafetyPrompt }) {
  const Icon = prompt.kind === "password" ? LockKeyhole : ShieldAlert;
  return (
    <AlertDialogHeader className="min-w-0 gap-1.5">
      <AlertDialogTitle className="flex min-w-0 items-start gap-2 text-left text-sm">
        <Icon
          aria-hidden
          className={cn(
            "mt-0.5 size-4 shrink-0",
            prompt.kind === "password" ? "text-brand" : "text-warning",
          )}
        />
        <span className="min-w-0 break-words">{prompt.title}</span>
      </AlertDialogTitle>
      <AlertDialogDescription asChild>
        <div className="flex min-w-0 flex-col gap-1 text-left text-xs">
          <ConnectionLine target={prompt.target} />
          {prompt.description && <p className="whitespace-pre-line">{prompt.description}</p>}
        </div>
      </AlertDialogDescription>
    </AlertDialogHeader>
  );
}

function AlertBody({ prompt }: { prompt: SafetyPrompt }) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  // Focus lands on the confirming button, so ↩ sends (also when the next queued prompt shows).
  useEffect(() => confirmRef.current?.focus(), []);
  return (
    <>
      <Header prompt={prompt} />
      <StatementList statements={prompt.statements} />
      <AlertDialogFooter className="py-1">
        <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
        <AlertDialogAction
          ref={confirmRef}
          data-safety-autofocus
          size="sm"
          onClick={() => answerSafetyPrompt(prompt.id, true)}
        >
          {prompt.confirmLabel}
        </AlertDialogAction>
      </AlertDialogFooter>
    </>
  );
}

/** What the system Touch ID dialog says: "Tabler is trying to <reason>." */
function touchIdReason(prompt: PasswordPrompt): string {
  return `unlock Safe Mode for “${prompt.target.name}”`;
}

function PasswordBody({ prompt }: { prompt: PasswordPrompt }) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const byName = prompt.secret === "name";
  useEffect(() => inputRef.current?.focus(), []);

  // Settings ▸ Security ▸ Unlock Safe Mode with Touch ID: offered once on its own, then on the
  // button; the typed password keeps working.
  const touchIdPref = usePrefs((p) => p.safeModeTouchId);
  const [touchId, setTouchId] = useState<"idle" | "asking" | "unavailable">("idle");
  const askTouchId = async () => {
    setTouchId("asking");
    const result = await authenticateOwner(touchIdReason(prompt));
    if (result === "ok") {
      answerSafetyPrompt(prompt.id, true);
      return;
    }
    setTouchId(result === "unavailable" ? "unavailable" : "idle");
    if (result === "unavailable") setError("Touch ID is not available. Type the password.");
    inputRef.current?.focus();
  };
  const askRef = useRef(askTouchId);
  askRef.current = askTouchId;
  useEffect(() => {
    if (touchIdPref) void askRef.current();
    // Once per prompt (the body is keyed by the prompt).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const retry = (message: string) => {
    setError(message);
    setChecking(false);
    // Keep the typed text selected: typing replaces it.
    queueMicrotask(() => inputRef.current?.select());
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (checking) return;
    if (byName) {
      if (value.trim() !== "" && value.trim() === prompt.target.name.trim()) {
        answerSafetyPrompt(prompt.id, true);
      } else {
        retry("That is not the name of this connection.");
      }
      return;
    }
    setChecking(true);
    setError(null);
    try {
      const ok = await ipc.safety.verifyPassword(prompt.target.profileId, value);
      if (ok) answerSafetyPrompt(prompt.id, true);
      else retry("Wrong password. Try again.");
    } catch (err) {
      retry(`Could not check the password: ${toIpcError(err).message}`);
    }
  };

  const label = byName ? "Connection name" : "Password";
  return (
    <form className="contents" onSubmit={(e) => void submit(e)}>
      <Header prompt={prompt} />
      <StatementList statements={prompt.statements} />
      <div className="flex flex-col gap-1">
        <Input
          ref={inputRef}
          data-safety-autofocus
          type={byName ? "text" : "password"}
          aria-label={label}
          placeholder={byName ? prompt.target.name : "Password"}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={value}
          aria-invalid={error !== null}
          aria-describedby={error ? `safety-error-${prompt.id}` : undefined}
          onChange={(e) => {
            setValue(e.target.value);
            setError(null);
          }}
          className="h-8 text-[13px]"
        />
        {error && (
          <p id={`safety-error-${prompt.id}`} role="alert" className="text-error text-xs">
            {error}
          </p>
        )}
      </div>
      <AlertDialogFooter className="py-1">
        {touchIdPref && touchId !== "unavailable" && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={touchId === "asking"}
            onClick={() => void askTouchId()}
            className="sm:mr-auto"
          >
            {touchId === "asking" ? (
              <Loader2 className="animate-spin" aria-hidden />
            ) : (
              <FingerprintPattern aria-hidden />
            )}
            Use Touch ID
          </Button>
        )}
        <AlertDialogCancel size="sm" type="button">
          Cancel
        </AlertDialogCancel>
        <Button type="submit" size="sm" disabled={checking || value === ""}>
          {checking && <Loader2 className="animate-spin" aria-hidden />}
          {prompt.confirmLabel}
        </Button>
      </AlertDialogFooter>
    </form>
  );
}

/**
 * The safe-mode dialog: Alert Mode's "Do you want to send these queries to the server?" (Send /
 * Commit / Continue, ↩ confirms, ⎋ cancels) or Safe Mode's password prompt, with the connection,
 * its tag and level, and the statements about to be sent.
 */
export function SafetyDialog({ prompt }: { prompt: SafetyPrompt | null }) {
  // Keep showing the last prompt while the dialog animates out.
  const [last, setLast] = useState(prompt);
  if (prompt && prompt !== last) setLast(prompt);
  const shown = prompt ?? last;
  const contentRef = useRef<HTMLDivElement>(null);

  // Only a fresh, plain ↩ confirms: not the auto-repeat of the ↩ / ⌘↩ that ran the query and
  // opened this dialog, which would otherwise "click" the focused Send button.
  const onKeyDownCapture = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Enter") return;
    if (e.repeat || e.metaKey || e.ctrlKey || e.altKey) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // ↩ sends from anywhere in the alert (the statement list, the dialog itself); a focused
    // button keeps its own ↩ (Cancel cancels).
    if (prompt?.kind !== "alert" || e.key !== "Enter" || e.nativeEvent.isComposing) return;
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    if ((e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    answerSafetyPrompt(prompt.id, true);
  };

  return (
    <AlertDialog
      open={prompt !== null}
      onOpenChange={(open) => {
        if (!open && prompt) answerSafetyPrompt(prompt.id, false);
      }}
    >
      <AlertDialogContent
        ref={contentRef}
        data-slot="safety-dialog"
        onKeyDownCapture={onKeyDownCapture}
        onKeyDown={onKeyDown}
        // Radix focuses Cancel; TablePlus's ↩ sends, so focus the confirming button (or the field).
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          contentRef.current?.querySelector<HTMLElement>("[data-safety-autofocus]")?.focus();
        }}
        className="grid-cols-1 gap-3 data-[size=default]:sm:max-w-lg"
      >
        {shown &&
          (shown.kind === "password" ? (
            <PasswordBody key={shown.id} prompt={shown} />
          ) : (
            <AlertBody key={shown.id} prompt={shown} />
          ))}
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * Renders the pending safe-mode prompt (requested by `guardExecution` and level changes). Mounted
 * once from AppShell; extra instances stay inert.
 */
export function SafetyHost() {
  const owner = useSingleHost(SAFETY_HOST);
  const prompt = useSafetyPromptStore((s) => s.queue[0] ?? null);
  useEffect(
    () => () => {
      // A host mounted in the same commit takes the queue over; otherwise nobody can answer the
      // pending prompts, so they are declined (nothing is sent).
      queueMicrotask(() => {
        if (!hasSingleHost(SAFETY_HOST)) abandonSafetyPrompts();
      });
    },
    [],
  );
  if (!owner) return null;
  return <SafetyDialog prompt={prompt} />;
}
