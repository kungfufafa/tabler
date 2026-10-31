import { useId } from "react";
import { Controller, type Path } from "react-hook-form";
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
import { AUTH_MECHANISMS, READ_PREFERENCES } from "./params";
import { parseMongoConnectionUrl, parseMongoUrl } from "./url";

const DIRECT: readonly (readonly [string, string])[] = [
  ["auto", "Automatic"],
  ["on", "On: this host only"],
  ["off", "Off: discover the replica set"],
];

/**
 * MongoDB's own rows: the connection string (a replica set, SRV, options the form lacks), the
 * sign-in mechanism and its database, the replica set, read preference, direct connection, the
 * app name, the server selection timeout and the schema sample's size.
 */
export function MongoFields({ form }: KindFieldsProps) {
  const { register, control, formState, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };

  // A pasted URL with credentials: the user goes to User and the password to Password, and the
  // URL is kept without them (it is saved in plain text).
  const takeCredentials = (text: string) => {
    const url = parseMongoUrl(text);
    const read = parseMongoConnectionUrl(text);
    if (!url || !read || (url.user === null && url.password === null)) return;
    if (url.user !== null) setValue("user", url.user, opts);
    if (read.password !== null) setValue("password", read.password, opts);
    const uri = (read.values.driverParams as { uri?: string } | undefined)?.uri;
    // A URL the fields hold whole is spread over them instead.
    if (!uri) {
      for (const [field, value] of Object.entries(read.values))
        setValue(field as Path<ConnectionFormValues>, value as never, opts);
    } else setValue("driverParams.uri", uri, opts);
  };

  const select = (
    name: Path<ConnectionFormValues>,
    label: string,
    items: readonly (readonly [string, string])[],
    fallback: string,
    hint?: string,
  ) => (
    <Row label={label} htmlFor={`${uid}-${name}`} hint={hint}>
      <Controller
        control={control}
        name={name}
        render={({ field }) => (
          <Select
            value={typeof field.value === "string" && field.value !== "" ? field.value : fallback}
            onValueChange={(v) =>
              field.onChange(name === "driverParams.directConnection" && v === "auto" ? "" : v)
            }
          >
            <SelectTrigger id={`${uid}-${name}`} className="w-64" aria-label={label}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {items.map(([id, text]) => (
                <SelectItem key={id} value={id}>
                  {text}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      />
    </Row>
  );

  const text = (
    key: string,
    label: string,
    placeholder: string,
    hint?: string,
    className = "font-mono",
  ) => (
    <Row label={label} htmlFor={`${uid}-${key}`} error={errors?.[key]?.message} hint={hint}>
      <Input
        id={`${uid}-${key}`}
        {...register(`driverParams.${key}` as Path<ConnectionFormValues>)}
        placeholder={placeholder}
        className={className}
        autoComplete="off"
        spellCheck={false}
      />
    </Row>
  );

  return (
    <>
      <Row
        label="URL"
        htmlFor={`${uid}-uri`}
        error={errors?.uri?.message}
        hint="Optional: a replica set, mongodb+srv:// or options the form lacks, in place of Host"
      >
        <Input
          id={`${uid}-uri`}
          {...register("driverParams.uri", {
            onBlur: (e: { target: { value: unknown } }) => takeCredentials(String(e.target.value)),
          })}
          placeholder="mongodb+srv://cluster0.example.mongodb.net/?retryWrites=true"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      {select("driverParams.authMechanism", "Sign-in", AUTH_MECHANISMS, "default")}
      {text(
        "authSource",
        "Auth database",
        "admin",
        "Where the user is defined ($external for X.509, PLAIN, OIDC)",
      )}
      {text("replicaSet", "Replica set", "")}
      {select("driverParams.readPreference", "Read from", READ_PREFERENCES, "primary")}
      {select(
        "driverParams.directConnection",
        "Direct",
        DIRECT,
        "auto",
        "Automatic: one host without a replica set name is reached directly, as mongosh does",
      )}
      {text("appName", "App name", "Tabler", undefined, "")}
      {text("serverSelectionTimeoutSecs", "Selection timeout", "seconds (the connect timeout)")}
      {text("sampleSize", "Sample size", "100", "Documents read to find a collection's fields")}
    </>
  );
}
