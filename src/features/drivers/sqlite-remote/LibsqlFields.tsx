import { useId } from "react";
import { Controller, type Path } from "react-hook-form";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { KindFieldsProps } from "../types";
import { parseLibsqlConnectionUrl, parseLibsqlUrl } from "./url";

/** LibSQL's own rows: the URL that replaces host and port, and Basic authentication. */
export function LibsqlFields({ form }: KindFieldsProps) {
  const { register, control, formState, watch, setValue } = form;
  const uid = useId();
  const basicAuth = watch("driverParams.basicAuth") === true;
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;

  // A pasted URL with a token or credentials is spread over the form: the token goes to Auth
  // Token and the credentials to User and Password, since the params are stored as plain text.
  const takeSecrets = (text: string) => {
    const parsed = parseLibsqlUrl(text);
    const read = parseLibsqlConnectionUrl(text);
    if (!parsed || !read || !parsed.hasSecret) return;
    const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };
    for (const [field, value] of Object.entries(read.values))
      if (field !== "user" || parsed.basic)
        setValue(field as Path<ConnectionFormValues>, value as never, opts);
    if (read.password !== null) setValue("password", read.password, opts);
  };

  return (
    <>
      <Row
        label="URL"
        htmlFor={`${uid}-url`}
        error={errors?.url?.message}
        hint="Optional: a server under a path (https://example.com/db) in place of host and port"
      >
        <Input
          id={`${uid}-url`}
          {...register("driverParams.url", { onBlur: (e) => takeSecrets(String(e.target.value)) })}
          placeholder="libsql://db-org.turso.io"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      <Row label="">
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name="driverParams.basicAuth"
            render={({ field }) => (
              <Checkbox
                checked={field.value === true}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Basic authentication"
              />
            )}
          />
          Basic authentication (a user and password instead of the token)
        </label>
      </Row>
      {basicAuth && (
        <Row label="User" htmlFor={`${uid}-user`} error={formState.errors.user?.message}>
          <Input
            id={`${uid}-user`}
            {...register("user")}
            className="font-mono"
            autoComplete="off"
          />
        </Row>
      )}
    </>
  );
}
