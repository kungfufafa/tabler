import { useId } from "react";
import { Controller } from "react-hook-form";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { inSentence } from "@/features/drivers/catalog";
import type { PasswordMode } from "@/lib/ipc";
import { SECRET_STORE } from "@/lib/platform";
import { PASSWORD_MODE_LABEL } from "../schema";
import { Row, type ConnectionForm } from "./Row";

/** The hint under the secret for each way of not storing it. */
const SECRET_HINT: Record<Exclude<PasswordMode, "keychain">, (secret: string) => string> = {
  ask: () => "Asked on every connect; kept in memory while connected",
  none: (secret) => `Connects without a ${secret}`,
  command: () => "The output of the command below, run on every connect; never stored",
};

const SECRET_PLACEHOLDER: Record<Exclude<PasswordMode, "keychain">, (secret: string) => string> = {
  ask: () => "Asked when connecting",
  none: (secret) => `No ${secret}`,
  command: () => "From the command",
};

/**
 * The secret (a password, a token or a key, as the driver's descriptor names it), how it is kept
 * (Keychain, asked, none, a command's output), "Clear" for a stored one, and the command's own
 * row in Command mode.
 */
export function SecretRow({
  form,
  hasStoredPassword,
  label = "Password",
  passwordModes,
  commandExample,
}: {
  form: ConnectionForm;
  hasStoredPassword: boolean;
  /** What the secret is called ("Password", "API Token"). */
  label?: string;
  /** The ways the driver keeps it (`DriverDescriptor.passwordModes`), in menu order. */
  passwordModes: readonly PasswordMode[];
  /** An example command printing the secret (Command mode's placeholder). */
  commandExample?: string;
}) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const uid = useId();
  const clearPassword = watch("clearPassword");
  const passwordMode = watch("passwordMode");
  const keychain = passwordMode === "keychain";
  const secret = inSentence(label);
  return (
    <>
      <Row
        label={label}
        htmlFor={`${uid}-password`}
        hint={
          !keychain
            ? SECRET_HINT[passwordMode](secret)
            : hasStoredPassword && !clearPassword
              ? "Stored in Keychain — leave empty to keep"
              : clearPassword
                ? `The stored ${secret} is removed on save`
                : `Stored in the ${SECRET_STORE}`
        }
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-password`}
            {...register("password")}
            type="password"
            placeholder={
              !keychain
                ? SECRET_PLACEHOLDER[passwordMode](secret)
                : hasStoredPassword && !clearPassword
                  ? "••••••••"
                  : ""
            }
            className="font-mono"
            autoComplete="new-password"
            disabled={clearPassword || !keychain}
          />
          <Controller
            control={control}
            name="passwordMode"
            render={({ field }) => (
              <Select
                value={field.value}
                onValueChange={(v) => {
                  const mode = passwordModes.find((m) => m === v);
                  if (!mode) return;
                  field.onChange(mode);
                  // Only the Keychain keeps a typed secret.
                  if (mode !== "keychain") setValue("password", "");
                }}
              >
                <SelectTrigger aria-label={`${label} storage`} className="w-40 shrink-0">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {passwordModes.map((m) => (
                    <SelectItem key={m} value={m}>
                      {PASSWORD_MODE_LABEL[m]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {hasStoredPassword && keychain && (
            <label
              className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-[11px] whitespace-nowrap"
              title={`Remove the ${secret} stored in the Keychain`}
            >
              <Controller
                control={control}
                name="clearPassword"
                render={({ field }) => (
                  <Checkbox
                    checked={field.value}
                    onCheckedChange={(v) => field.onChange(v === true)}
                  />
                )}
              />
              Clear
            </label>
          )}
        </div>
      </Row>
      {passwordMode === "command" && (
        <Row
          label="Command"
          htmlFor={`${uid}-command`}
          error={errors.passwordCommand?.message}
          hint={
            <>
              Runs in your login shell (<span className="font-mono">$SHELL -lc</span>, 30 s limit);
              its output is the {secret}
            </>
          }
        >
          <Textarea
            id={`${uid}-command`}
            {...register("passwordCommand")}
            placeholder={commandExample}
            rows={2}
            spellCheck={false}
            autoComplete="off"
            className="min-h-0 resize-none font-mono text-xs"
            aria-invalid={errors.passwordCommand ? true : undefined}
          />
        </Row>
      )}
    </>
  );
}
