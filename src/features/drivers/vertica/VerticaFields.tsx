import { useId } from "react";
import { Controller, type Path } from "react-hook-form";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { KindFieldsProps } from "../types";
import { AUTH_METHODS, DEFAULT_CLIENT_LABEL, verticaParamsOf } from "./params";

/**
 * Vertica's own rows: what the password field holds (a password or an OAuth access token), the
 * client label `v_monitor.sessions` shows, the workload a routing rule sends the sessions by, and
 * Manual commit for the editor (the app's own connections always commit each statement).
 */
export function VerticaFields({ form }: KindFieldsProps) {
  const { register, control, formState, watch } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const auth = verticaParamsOf(watch("driverParams")).auth;

  const textRow = (key: string, label: string, placeholder: string, hint: string) => (
    <Row label={label} htmlFor={`${uid}-${key}`} error={errors?.[key]?.message} hint={hint}>
      <Input
        id={`${uid}-${key}`}
        {...register(`driverParams.${key}` as Path<ConnectionFormValues>)}
        placeholder={placeholder}
        className="font-mono"
        autoComplete="off"
        spellCheck={false}
      />
    </Row>
  );

  return (
    <>
      <Row
        label="Authentication"
        htmlFor={`${uid}-auth`}
        error={errors?.auth?.message}
        hint={
          auth === "oauth"
            ? "The password is the token, sent over TLS only: Command Line can run your identity provider's CLI to get a fresh one"
            : "Hashed as the server asks (SHA-512 or MD5); sent as it is only over TLS, for LDAP users"
        }
      >
        <Controller
          control={control}
          name="driverParams.auth"
          render={({ field }) => (
            <Select
              value={verticaParamsOf({ auth: field.value }).auth}
              onValueChange={(v) => field.onChange(v)}
            >
              <SelectTrigger id={`${uid}-auth`} className="w-64" aria-label="Authentication">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {AUTH_METHODS.map(([id, text]) => (
                  <SelectItem key={id} value={id}>
                    {text}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {textRow(
        "clientLabel",
        "Client label",
        DEFAULT_CLIENT_LABEL,
        "Optional: what v_monitor.sessions shows for these sessions",
      )}
      {textRow(
        "workload",
        "Workload",
        "the server's default",
        "Optional: the workload a routing rule sends the sessions by",
      )}
      <Row
        label=""
        hint="The editor's statements then wait for COMMIT (not on a read-only connection)"
      >
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name={"driverParams.manualCommit" as Path<ConnectionFormValues>}
            render={({ field }) => (
              <Checkbox
                checked={field.value === true}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Manual commit"
              />
            )}
          />
          Manual commit
        </label>
      </Row>
    </>
  );
}
