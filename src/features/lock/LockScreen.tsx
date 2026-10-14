import { FingerprintPattern, Loader2, LockKeyhole } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { usePrefs } from "@/stores/prefs";
import { canAttempt, cooldownMessage } from "./machine";
import { unlockWithBiometric, unlockWithPasscode, useLockStore } from "./store";

/**
 * Events stopped at the lock screen so the app behind it never sees them: a dialog left open
 * underneath would otherwise close on a click or ⎋ here, or pull the focus back (Radix's
 * outside-click, Escape and focus-trap listeners sit on the document).
 */
const SHIELDED_EVENTS = ["pointerdown", "mousedown", "focusin", "keydown", "keyup"] as const;

/**
 * Marks every other child of `<body>` inert and hidden from assistive tech while `root` is
 * mounted, including portals opened later (menus, toasts). Restores only what it changed.
 */
function useInertSiblings(root: HTMLElement | null): void {
  useLayoutEffect(() => {
    if (!root) return;
    const changed = new Map<Element, { inert: boolean; hidden: boolean }>();
    const shield = (el: Element) => {
      if (el === root || changed.has(el) || !(el instanceof HTMLElement)) return;
      const inert = !el.hasAttribute("inert");
      const hidden = !el.hasAttribute("aria-hidden");
      if (inert) el.setAttribute("inert", "");
      if (hidden) el.setAttribute("aria-hidden", "true");
      changed.set(el, { inert, hidden });
    };
    Array.from(document.body.children).forEach(shield);
    const observer = new MutationObserver((records) => {
      for (const r of records) r.addedNodes.forEach((n) => n instanceof Element && shield(n));
    });
    observer.observe(document.body, { childList: true });
    return () => {
      observer.disconnect();
      for (const [el, { inert, hidden }] of changed) {
        if (inert) el.removeAttribute("inert");
        if (hidden) el.removeAttribute("aria-hidden");
      }
    };
  }, [root]);
}

/** The cooldown's countdown, re-rendered while it runs. */
function useCooldown(): number {
  const until = useLockStore((s) => s.attempts.cooldownUntil);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (until === null) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(timer);
  }, [until]);
  return until === null ? 0 : Math.max(0, until - now);
}

/**
 * The full-window lock screen (Settings ▸ Security ▸ Passcode): the app behind it is blurred and
 * inert, nothing of it is readable or reachable. Unlocks with the passcode or, with "Unlock
 * Passcode with Touch ID", Touch ID / the account password (offered once per lock, when the
 * window has the focus). 5 wrong passcodes → 30 s cooldown.
 */
export function LockScreen() {
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  useInertSiblings(root);
  const touchIdPref = usePrefs((p) => p.passcodeTouchId);
  const biometrics = useLockStore((s) => s.biometrics);
  const lockCount = useLockStore((s) => s.lockCount);
  const canTouchId = touchIdPref && biometrics;
  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<"passcode" | "touchId" | null>(null);
  const cooldown = useCooldown();
  const coolingDown = cooldown > 0;

  // Native listeners: React's synthetic stopPropagation runs after the document's listeners.
  useEffect(() => {
    if (!root) return;
    const stop = (e: Event) => e.stopPropagation();
    for (const name of SHIELDED_EVENTS) root.addEventListener(name, stop);
    return () => {
      for (const name of SHIELDED_EVENTS) root.removeEventListener(name, stop);
    };
  }, [root]);

  useEffect(() => {
    if (root) inputRef.current?.focus();
  }, [root, lockCount]);

  const touchId = async () => {
    if (busy) return;
    setBusy("touchId");
    const result = await unlockWithBiometric();
    setBusy(null);
    if (!result.ok) {
      if (result.message) setMessage(result.message);
      inputRef.current?.focus();
    }
  };

  // Touch ID once per lock: right away when the window has the focus, else on its next focus
  // (a lock while the user is in another app must not pop the system dialog over it).
  const touchIdRef = useRef(touchId);
  touchIdRef.current = touchId;
  useEffect(() => {
    if (!canTouchId) return;
    if (document.hasFocus()) {
      void touchIdRef.current();
      return;
    }
    const onFocus = () => void touchIdRef.current();
    window.addEventListener("focus", onFocus, { once: true });
    return () => window.removeEventListener("focus", onFocus);
  }, [canTouchId, lockCount]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy || value === "") return;
    if (!canAttempt(useLockStore.getState().attempts, Date.now())) return;
    setBusy("passcode");
    setMessage(null);
    const result = await unlockWithPasscode(value);
    setBusy(null);
    if (result.ok) return;
    setMessage(result.message);
    setValue("");
    queueMicrotask(() => inputRef.current?.focus());
  };

  const shown = coolingDown ? cooldownMessage(cooldown) : message;
  return (
    <div
      ref={setRoot}
      data-slot="lock-screen"
      role="dialog"
      aria-modal="true"
      aria-labelledby="lock-screen-title"
      // Above every portal (dialogs, menus, toasts); body may carry Radix's pointer-events: none.
      className="bg-background/55 fixed inset-0 z-[2147483000] flex items-center justify-center backdrop-blur-3xl backdrop-saturate-50"
      style={{ pointerEvents: "auto" }}
    >
      {/* The window still drags by its top edge. */}
      <div data-tauri-drag-region className="absolute inset-x-0 top-0 h-10" />
      <form
        onSubmit={(e) => void submit(e)}
        className="bg-popover text-popover-foreground border-border flex w-72 flex-col items-center gap-3 rounded-xl border px-6 py-6 shadow-xl"
      >
        <div className="bg-brand-soft text-brand flex size-11 items-center justify-center rounded-full">
          <LockKeyhole className="size-5" aria-hidden />
        </div>
        <div className="flex flex-col items-center gap-0.5 text-center">
          <h2 id="lock-screen-title" className="text-sm font-semibold">
            Tabler is locked
          </h2>
          <p className="text-muted-foreground text-xs">Enter your passcode to unlock.</p>
        </div>
        <Input
          ref={inputRef}
          type="password"
          aria-label="Passcode"
          placeholder="Passcode"
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={value}
          disabled={coolingDown}
          aria-invalid={shown ? true : undefined}
          aria-describedby={shown ? "lock-screen-message" : undefined}
          onChange={(e) => {
            setValue(e.target.value);
            setMessage(null);
          }}
          className="h-8 text-center text-[13px]"
        />
        <p
          id="lock-screen-message"
          role="alert"
          className="text-error min-h-4 text-center text-xs leading-4"
        >
          {shown}
        </p>
        <div className="flex w-full flex-col gap-1.5">
          <Button
            type="submit"
            size="sm"
            disabled={busy !== null || value === "" || coolingDown}
            className="w-full"
          >
            {busy === "passcode" && <Loader2 className="animate-spin" aria-hidden />}
            Unlock
          </Button>
          {canTouchId && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy !== null}
              onClick={() => void touchId()}
              className="w-full"
            >
              {busy === "touchId" ? (
                <Loader2 className="animate-spin" aria-hidden />
              ) : (
                <FingerprintPattern aria-hidden />
              )}
              Use Touch ID
            </Button>
          )}
        </div>
      </form>
    </div>
  );
}
