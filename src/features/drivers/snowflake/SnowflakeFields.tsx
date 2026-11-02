import { FolderOpen, Loader2 } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { Controller, type Path } from "react-hook-form";
import { Button } from "@/components/ui/button";
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
import { pickFile } from "@/features/connections/rows/files";
import { Row } from "@/features/connections/rows/Row";
import { ExtraSecretRows, SecretRow } from "@/features/connections/rows/SecretRow";
import { UserRow } from "@/features/connections/rows/UserRow";
import type { ConnectionFormValues } from "@/features/connections/schema";
import { ipc, toIpcError } from "@/lib/ipc";
import { DESCRIPTORS } from "../catalog";
import type { KindFieldsProps } from "../types";
import { AUTH_METHODS, SECRET_LABEL, snowflakeParamsOf } from "./params";
import { type FormCall, followAuth, type Listing, missingForList, utilInput } from "./picker";

/** The secret's storage modes by sign-in: a token or password is never "no password". */
const SECRET_MODES = DESCRIPTORS.snowflake.passwordModes.filter((m) => m !== "none");

const COMMAND_EXAMPLE: Record<string, string> = {
  oauth: "my-idp-cli token --audience snowflake",
  pat: "op read op://vault/snowflake/pat",
  password: "op read op://vault/snowflake/password",
};

/**
 * Snowflake's rows: the account identifier, the user, the sign-in (a key pair's private key file
 * with Choose… and its fingerprint to compare with `DESC USER`, and the key's passphrase; or the
 * PAT, OAuth token or password as the secret, with MFA for a password account), then the
 * warehouse, role and database with the lists the sign-in sees, the schema, an endpoint of its
 * own, session parameters and the statement timeout.
 */
export function SnowflakeFields({ form, hasStoredPassword }: KindFieldsProps) {
  const { register, control, formState, watch, getValues, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const params = snowflakeParamsOf(watch("driverParams"));
  const [lists, setLists] = useState<Partial<Record<FormCall, Listing>>>({});
  const [fingerprint, setFingerprint] = useState<Listing>({ state: "idle" });

  const setList = (call: FormCall, l: Listing) => setLists((all) => ({ ...all, [call]: l }));

  const list = async (call: Exclude<FormCall, "keyFingerprint">) => {
    const values = getValues();
    const missing = missingForList(values, hasStoredPassword);
    if (missing) return setList(call, { state: "failed", message: missing });
    setList(call, { state: "loading" });
    try {
      const names = await ipc.driverUtil<string[]>(utilInput(values), call);
      setList(call, {
        state: "done",
        items: Array.isArray(names) ? names.filter((n) => typeof n === "string") : [],
      });
    } catch (err) {
      setList(call, { state: "failed", message: toIpcError(err).message });
    }
  };

  const readFingerprint = async () => {
    const values = getValues();
    const path = snowflakeParamsOf(values.driverParams).privateKeyPath;
    if (path === "")
      return setFingerprint({ state: "failed", message: "Choose the private key file first" });
    setFingerprint({ state: "loading" });
    try {
      // Read from the file the form names: nothing is sent.
      const out = await ipc.driverUtil<{ fingerprint?: unknown }>(
        utilInput(values),
        "keyFingerprint",
        { privateKeyPath: path },
      );
      const fp = typeof out?.fingerprint === "string" ? out.fingerprint : "";
      setFingerprint({ state: "done", items: [fp] });
    } catch (err) {
      setFingerprint({ state: "failed", message: toIpcError(err).message });
    }
  };

  const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };

  const status = (l: Listing | undefined, noun: string, fallback: ReactNode): ReactNode =>
    l?.state === "done"
      ? l.items.length === 0
        ? `No ${noun}s visible to this sign-in`
        : `${l.items.length} ${noun}${l.items.length === 1 ? "" : "s"}: pick one`
      : l?.state === "failed"
        ? l.message
        : fallback;

  const button = (label: string, loading: boolean, onClick: () => void) => (
    <Button type="button" variant="outline" size="sm" disabled={loading} onClick={onClick}>
      {loading && <Loader2 className="size-3.5 animate-spin" />}
      {label}
    </Button>
  );

  const text = (key: string, label: string, placeholder: string, hint?: ReactNode) => (
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

  /** A name the sign-in may pick from a list (`field`: a param or the database). */
  const picked = (
    call: Exclude<FormCall, "keyFingerprint">,
    field: Path<ConnectionFormValues>,
    label: string,
    noun: string,
    placeholder: string,
    fallback: string,
    error: string | undefined,
  ) => {
    const l = lists[call];
    return (
      <Row
        label={label}
        htmlFor={`${uid}-${call}`}
        error={error}
        hint={<span role="status">{status(l, noun, fallback)}</span>}
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-${call}`}
            {...register(field)}
            list={`${uid}-${call}-list`}
            placeholder={placeholder}
            className="font-mono"
            autoComplete="off"
            spellCheck={false}
          />
          {button(`List ${noun}s`, l?.state === "loading", () => void list(call))}
        </div>
        <datalist id={`${uid}-${call}-list`} data-testid={`snowflake-${call}`}>
          {l?.state === "done" && l.items.map((n) => <option key={n} value={n} />)}
        </datalist>
      </Row>
    );
  };

  return (
    <>
      {text(
        "account",
        "Account",
        "myorg-myaccount",
        "orgname-account_name, or the locator with its region (xy12345.us-east-2.aws); a pasted URL is fine",
      )}
      <UserRow form={form} placeholder="TABLER_USER" />
      <Row label="Sign in with" htmlFor={`${uid}-auth`}>
        <Controller
          control={control}
          name={"driverParams.auth" as Path<ConnectionFormValues>}
          render={({ field }) => (
            <Select
              value={snowflakeParamsOf({ auth: field.value }).auth}
              onValueChange={(v) => {
                field.onChange(v);
                followAuth(form, v);
                setFingerprint({ state: "idle" });
              }}
            >
              <SelectTrigger id={`${uid}-auth`} className="w-72" aria-label="Sign in with">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {AUTH_METHODS.map(([id, label]) => (
                  <SelectItem key={id} value={id}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {params.auth === "keyPair" ? (
        <>
          <Row
            label="Private key"
            htmlFor={`${uid}-privateKeyPath`}
            error={errors?.privateKeyPath?.message}
            hint={
              <span role="status">
                {fingerprint.state === "done"
                  ? `Fingerprint ${fingerprint.items[0] ?? ""}: DESC USER shows it as RSA_PUBLIC_KEY_FP`
                  : fingerprint.state === "failed"
                    ? fingerprint.message
                    : "The PKCS#8 PEM file (rsa_key.p8), read when connecting and never stored"}
              </span>
            }
          >
            <div className="flex gap-2">
              <Input
                id={`${uid}-privateKeyPath`}
                {...register("driverParams.privateKeyPath" as Path<ConnectionFormValues>)}
                placeholder="/path/to/rsa_key.p8"
                className="font-mono"
                autoComplete="off"
                spellCheck={false}
              />
              <Button
                type="button"
                variant="outline"
                title="Choose the private key file"
                onClick={async () => {
                  const p = await pickFile("Snowflake private key", [
                    { name: "PKCS#8 key", extensions: ["p8", "pem", "key"] },
                  ]);
                  if (p) {
                    setValue("driverParams.privateKeyPath" as Path<ConnectionFormValues>, p, opts);
                    setFingerprint({ state: "idle" });
                  }
                }}
              >
                <FolderOpen /> Choose…
              </Button>
              {button("Fingerprint", fingerprint.state === "loading", () => void readFingerprint())}
            </div>
          </Row>
          {/* The shared `extraSecrets` row (the key's passphrase): only a key pair has one. */}
          <ExtraSecretRows form={form} secrets={DESCRIPTORS.snowflake.extraSecrets} />
        </>
      ) : (
        <SecretRow
          form={form}
          hasStoredPassword={hasStoredPassword}
          label={SECRET_LABEL[params.auth]}
          passwordModes={SECRET_MODES}
          commandExample={COMMAND_EXAMPLE[params.auth]}
        />
      )}
      {params.auth === "password" && (
        <Row
          label=""
          hint="Every sign-in waits for the push to be approved (up to 2 minutes); not for unattended use"
        >
          <label className="flex h-8 items-center gap-2 text-xs">
            <Controller
              control={control}
              name={"driverParams.mfa" as Path<ConnectionFormValues>}
              render={({ field }) => (
                <Checkbox
                  checked={field.value === true}
                  onCheckedChange={(v) => field.onChange(v === true)}
                  aria-label="MFA push"
                />
              )}
            />
            The account asks MFA (Duo push)
          </label>
        </Row>
      )}
      {picked(
        "listWarehouses",
        "driverParams.warehouse" as Path<ConnectionFormValues>,
        "Warehouse",
        "warehouse",
        "the user's default",
        "Optional: queries need one; browsing the catalog does not",
        errors?.warehouse?.message,
      )}
      {picked(
        "listRoles",
        "driverParams.role" as Path<ConnectionFormValues>,
        "Role",
        "role",
        "the user's default",
        "Optional",
        errors?.role?.message,
      )}
      {picked(
        "listDatabases",
        "database",
        "Database",
        "database",
        "the user's default",
        "Optional: one database per connection (switching reconnects)",
        formState.errors.database?.message,
      )}
      {text("schema", "Schema", "PUBLIC", "Optional: the schema the session starts in")}
      {text(
        "baseUrl",
        "Endpoint",
        "https://<account>.snowflakecomputing.com",
        "Optional: a PrivateLink host or a proxy (https://host only)",
      )}
      <Row
        label="Session parameters"
        htmlFor={`${uid}-sessionParameters`}
        error={errors?.sessionParameters?.message}
        hint="One NAME = value per line (QUERY_TAG = tabler); set at every sign-in"
      >
        <Textarea
          id={`${uid}-sessionParameters`}
          {...register("driverParams.sessionParameters" as Path<ConnectionFormValues>)}
          placeholder={"QUERY_TAG = tabler\nTIMEZONE = UTC"}
          className="min-h-16 font-mono text-xs"
          spellCheck={false}
        />
      </Row>
      {text(
        "statementTimeoutSecs",
        "Statement timeout",
        "the account's",
        "Seconds (STATEMENT_TIMEOUT_IN_SECONDS); empty for the account's",
      )}
    </>
  );
}
