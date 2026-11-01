import { useEffect, useId, useRef } from "react";
import { Controller, type Path } from "react-hook-form";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { KindFieldsProps } from "../types";
import { SESSION_TIMEOUT } from "./params";

/** The HTTP interface's ports: plain, and HTTPS (the descriptor's `defaultPort` / `tlsPort`). */
export const HTTP_PORT = "8123";
export const HTTPS_PORT = "8443";

/**
 * ClickHouse's own rows: signing in with the client certificate (an X.509 user), the roles every
 * request acts as, the settings sent with every request, and how long the editor's session (its
 * `SET`s and temporary tables) lasts while idle. Turning SSL on or off moves a default port to
 * the other one (8123 for HTTP, 8443 for HTTPS); a port the user typed stays.
 */
export function ClickhouseFields({ form }: KindFieldsProps) {
  const { register, control, formState, watch, getValues, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const tls = watch("ssl") !== "disable";
  const wasTls = useRef(tls);

  useEffect(() => {
    if (wasTls.current === tls) return;
    wasTls.current = tls;
    const port = getValues("port").trim();
    const [from, to] = tls ? [HTTP_PORT, HTTPS_PORT] : [HTTPS_PORT, HTTP_PORT];
    if (port === from) setValue("port", to, { shouldDirty: true });
  }, [tls, getValues, setValue]);

  const textRow = (key: string, label: string, placeholder: string, hint?: string) => (
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
        label=""
        error={errors?.certificateAuth?.message}
        hint="For a user IDENTIFIED WITH ssl_certificate: needs HTTPS and the Key… and Cert… files; no password is sent"
      >
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name={"driverParams.certificateAuth" as Path<ConnectionFormValues>}
            render={({ field }) => (
              <Checkbox
                checked={field.value === true}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Sign in with the client certificate"
              />
            )}
          />
          Sign in with the client certificate
        </label>
      </Row>
      {textRow(
        "roles",
        "Roles",
        "analyst, reporting",
        "Optional: roles every request acts as, comma-separated (ClickHouse 24.4 and later)",
      )}
      <Row
        label="Settings"
        htmlFor={`${uid}-settings`}
        error={errors?.settings?.message}
        hint="Optional: one name = value per line, sent with every request (max_execution_time = 60)"
      >
        <Textarea
          id={`${uid}-settings`}
          {...register("driverParams.settings" as Path<ConnectionFormValues>)}
          placeholder="max_execution_time = 60"
          className="min-h-16 font-mono text-xs"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      {textRow(
        "sessionTimeoutSecs",
        "Session idle",
        "60",
        `Optional: seconds the editor's session keeps its SETs and temporary tables while idle, ${SESSION_TIMEOUT.min} to ${SESSION_TIMEOUT.max}`,
      )}
    </>
  );
}
