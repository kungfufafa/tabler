import { Loader2 } from "lucide-react";
import { useId, useState, type FormEvent } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import { ipc, toIpcError } from "@/lib/ipc";
import { shortcutFor } from "@/lib/keys/keymap";
import { AUTO_LOCK_MINUTES, setPref, usePrefs, type AutoLockMinutes } from "@/stores/prefs";
import { AUTO_LOCK_LABEL, newPasscodeError } from "./passcodeRules";
import { setPasscodeEnabled, useLockStore } from "./store";

const UNAVAILABLE = "Touch ID is not available on this Mac; the account password is asked instead.";

/** Security ▸ Unlock Safe Mode with Touch ID. */
export function SafeModeTouchIdRow() {
  const on = usePrefs((p) => p.safeModeTouchId);
  const checked = useLockStore((s) => s.checked);
  const biometrics = useLockStore((s) => s.biometrics);
  return (
    <SettingRow
      id="pref-safe-mode-touch-id"
      label="Unlock Safe Mode with Touch ID"
      description={
        checked && !biometrics
          ? UNAVAILABLE
          : "Safe Mode prompts and Copy as URL accept Touch ID (or your Mac's password) instead of the connection's password."
      }
    >
      <Switch
        id="pref-safe-mode-touch-id"
        checked={on}
        onCheckedChange={(v) => setPref("safeModeTouchId", v)}
      />
    </SettingRow>
  );
}

export type PasscodeDialogMode = "set" | "change" | "disable";

const DIALOG_TEXT: Record<PasscodeDialogMode, { title: string; description: string; ok: string }> =
  {
    set: {
      title: "Enable Passcode",
      description:
        "Tabler asks for it at startup and after you have been away. It cannot be recovered: keep it safe.",
      ok: "Enable",
    },
    change: {
      title: "Change Passcode",
      description: "Enter the current passcode, then the new one twice.",
      ok: "Change",
    },
    disable: {
      title: "Turn Off Passcode",
      description: "Enter the current passcode to stop locking Tabler.",
      ok: "Turn Off",
    },
  };

/** Set / change / turn off the passcode. */
export function PasscodeDialog({
  mode,
  onClose,
}: {
  mode: PasscodeDialogMode | null;
  onClose: () => void;
}) {
  // Keep the last mode while the dialog animates out.
  const [shown, setShown] = useState(mode);
  if (mode && mode !== shown) setShown(mode);
  return (
    <Dialog open={mode !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-sm" data-scope="dialog">
        {shown && <PasscodeForm key={`${shown}:${mode !== null}`} mode={shown} onDone={onClose} />}
      </DialogContent>
    </Dialog>
  );
}

function PasscodeForm({ mode, onDone }: { mode: PasscodeDialogMode; onDone: () => void }) {
  const id = useId();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const text = DIALOG_TEXT[mode];
  const asksCurrent = mode !== "set";
  const asksNew = mode !== "disable";

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    if (asksCurrent && current === "") {
      setError("Enter the current passcode.");
      return;
    }
    if (asksNew) {
      const problem = newPasscodeError(next, confirm);
      if (problem) {
        setError(problem);
        return;
      }
    }
    setBusy(true);
    setError(null);
    try {
      await ipc.security.setPasscode(asksCurrent ? current : null, asksNew ? next : null);
      setPasscodeEnabled(mode !== "disable");
      toast.success(
        mode === "set"
          ? "Passcode enabled"
          : mode === "change"
            ? "Passcode changed"
            : "Passcode turned off",
      );
      onDone();
    } catch (err) {
      const e = toIpcError(err);
      setError(e.code === "AUTH_FAILED" ? "Wrong passcode." : e.message);
      setBusy(false);
    }
  };

  const field = (
    key: string,
    label: string,
    value: string,
    set: (v: string) => void,
    autoFocus: boolean,
  ) => (
    <Input
      id={`${id}-${key}`}
      type="password"
      aria-label={label}
      placeholder={label}
      autoComplete="off"
      autoFocus={autoFocus}
      value={value}
      onChange={(e) => {
        set(e.target.value);
        setError(null);
      }}
      className="h-8 text-[13px]"
    />
  );

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle>{text.title}</DialogTitle>
        <DialogDescription>{text.description}</DialogDescription>
      </DialogHeader>
      <div className="flex flex-col gap-2">
        {asksCurrent && field("current", "Current passcode", current, setCurrent, true)}
        {asksNew &&
          field("new", mode === "set" ? "Passcode" : "New passcode", next, setNext, !asksCurrent)}
        {asksNew && field("confirm", "Confirm passcode", confirm, setConfirm, false)}
        {error && (
          <p role="alert" className="text-error text-[11px]">
            {error}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button type="button" variant="ghost" size="sm" onClick={onDone}>
          Cancel
        </Button>
        <Button
          type="submit"
          size="sm"
          variant={mode === "disable" ? "destructive" : "default"}
          disabled={busy}
        >
          {busy && <Loader2 className="animate-spin" aria-hidden />}
          {text.ok}
        </Button>
      </DialogFooter>
    </form>
  );
}

/**
 * Security ▸ Passcode (TablePlus): Enable Passcode, Change Passcode…, Unlock Passcode with Touch
 * ID, Auto lock if away for. The passcode is asked at startup and after being away.
 */
export function PasscodeSettings() {
  const enabled = useLockStore((s) => s.enabled);
  const checked = useLockStore((s) => s.checked);
  const biometrics = useLockStore((s) => s.biometrics);
  const touchId = usePrefs((p) => p.passcodeTouchId);
  const minutes = usePrefs((p) => p.autoLockMinutes);
  const [dialog, setDialog] = useState<PasscodeDialogMode | null>(null);
  const lockKeys = shortcutFor("app.lock");
  return (
    <SettingGroup title="Passcode">
      <SettingRow
        id="pref-passcode"
        label="Enable Passcode"
        description={
          <>
            Asked at startup and after being away. Lock now from Window ▸ Lock Tabler
            {lockKeys && (
              <Kbd combo={lockKeys} className="mx-0.5 h-4 min-w-4 align-[-2px] text-[10px]" />
            )}
            .
          </>
        }
      >
        <Switch
          id="pref-passcode"
          checked={enabled}
          onCheckedChange={(v) => setDialog(v ? "set" : "disable")}
        />
      </SettingRow>
      <SettingRow id="pref-passcode-change" label="Change Passcode…">
        <Button
          id="pref-passcode-change"
          variant="outline"
          size="xs"
          disabled={!enabled}
          onClick={() => setDialog("change")}
        >
          Change Passcode…
        </Button>
      </SettingRow>
      <SettingRow
        id="pref-passcode-touch-id"
        label="Unlock Passcode with Touch ID"
        description={checked && !biometrics ? UNAVAILABLE : undefined}
      >
        <Switch
          id="pref-passcode-touch-id"
          checked={touchId}
          disabled={!enabled}
          onCheckedChange={(v) => setPref("passcodeTouchId", v)}
        />
      </SettingRow>
      <SettingRow
        id="pref-auto-lock"
        label="Auto lock if away for"
        description="No keyboard or mouse activity in Tabler for this long locks it. The Mac sleeping or its screen locking locks it right away."
      >
        <Select
          value={String(minutes)}
          disabled={!enabled}
          onValueChange={(v) => setPref("autoLockMinutes", Number(v) as AutoLockMinutes)}
        >
          <SelectTrigger id="pref-auto-lock" size="sm" className="h-7 w-32 rounded-md text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent data-scope="dialog">
            {AUTO_LOCK_MINUTES.map((m) => (
              <SelectItem key={m} value={String(m)} className="text-xs">
                {AUTO_LOCK_LABEL[m]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </SettingRow>
      <PasscodeDialog mode={dialog} onClose={() => setDialog(null)} />
    </SettingGroup>
  );
}
