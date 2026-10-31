import { useEffect, useId, useState, type ReactNode } from "react";
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
import { SecretRow } from "@/features/connections/rows/SecretRow";
import { SslRows } from "@/features/connections/rows/SslRows";
import { UserRow } from "@/features/connections/rows/UserRow";
import { formToProfileInput, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc, toIpcError, type ProfileInput } from "@/lib/ipc";
import { DESCRIPTORS } from "../catalog";
import type { KindFieldsProps } from "../types";
import { DYNAMO_SSL_MODES } from "./form";
import {
  AUTH_SOURCES,
  DEFAULT_PAGE_REQUESTS,
  DEFAULT_SAMPLE_SIZE,
  dynamoFormOf,
  dynamoParamsOf,
  formTlsApplies,
  LOCAL_ENDPOINT,
  usesKeyId,
  usesSecret,
} from "./params";

/** No saved profile's id reaches the form's calls: they need no secret. */
const UNSAVED = "00000000-0000-0000-0000-000000000000";

/**
 * The profile the form would connect with, for the form's calls (`regions`, `awsProfiles`), with
 * no secret and no way to get one: the backend resolves a Command Line secret before any util
 * call, and opening the form must not run the user's `credential_process` (an SSO login, an MFA
 * push) for a list that needs none.
 */
function utilInput(v: ConnectionFormValues): ProfileInput {
  const input = formToProfileInput(v, null, UNSAVED);
  input.profile = {
    ...input.profile,
    options: {
      ...input.profile.options,
      passwordMode: "none",
      passwordCommand: null,
      driverParams: { ...dynamoParamsOf(dynamoFormOf(v.driverParams)) },
    },
  };
  input.password = null;
  input.sshPassword = null;
  input.sshPassphrase = null;
  input.keyPassphrase = null;
  input.sessionToken = null;
  return input;
}

type Listing =
  | { state: "idle" | "loading" }
  | { state: "done"; names: string[] }
  | { state: "failed"; message: string };

/**
 * DynamoDB's rows, by the credential source: the access key ID and secret access key (the
 * secret's Command Line prints `credential_process` JSON for the Command source), the AWS
 * profile with this machine's profile names, the region with AWS's region names, an endpoint of
 * its own (DynamoDB Local, LocalStack, a VPC or FIPS endpoint) with its TLS, and the grid's
 * settings.
 */
export function DynamoFields({ form, hasStoredPassword }: KindFieldsProps) {
  const { register, control, formState, watch, getValues, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const params = dynamoFormOf(watch("driverParams"));
  const { passwordModes } = DESCRIPTORS.dynamodb;
  const [regions, setRegions] = useState<string[]>([]);
  const [profiles, setProfiles] = useState<Listing>({ state: "idle" });

  // The region names are the backend's list; a name it lacks can still be typed.
  useEffect(() => {
    let live = true;
    ipc
      .driverUtil<string[]>(utilInput(getValues()), "regions")
      .then((r) => live && setRegions(Array.isArray(r) ? r : []))
      .catch(() => live && setRegions([]));
    return () => {
      live = false;
    };
  }, [getValues]);

  // This machine's profile names, read once the source is a profile.
  const profileSource = params.auth === "profile";
  useEffect(() => {
    if (!profileSource) return;
    let live = true;
    setProfiles({ state: "loading" });
    ipc
      .driverUtil<string[]>(utilInput(getValues()), "awsProfiles")
      .then(
        (names) => live && setProfiles({ state: "done", names: Array.isArray(names) ? names : [] }),
      )
      .catch(
        (err: unknown) =>
          live && setProfiles({ state: "failed", message: toIpcError(err).message }),
      );
    return () => {
      live = false;
    };
  }, [profileSource, getValues]);

  const text = (
    key: string,
    label: string,
    placeholder: string,
    hint?: ReactNode,
    list?: string,
  ) => (
    <Row label={label} htmlFor={`${uid}-${key}`} error={errors?.[key]?.message} hint={hint}>
      <Input
        id={`${uid}-${key}`}
        {...register(`driverParams.${key}` as Path<ConnectionFormValues>)}
        placeholder={placeholder}
        className="font-mono"
        autoComplete="off"
        spellCheck={false}
        list={list}
      />
    </Row>
  );

  const check = (key: "consistentReads" | "consumedCapacity", label: string) => (
    <label className="flex h-8 items-center gap-2 text-xs">
      <Controller
        control={control}
        name={`driverParams.${key}`}
        render={({ field }) => (
          <Checkbox
            checked={field.value === true}
            onCheckedChange={(v) => field.onChange(v === true)}
            aria-label={label}
          />
        )}
      />
      {label}
    </label>
  );

  const profileHint =
    profiles.state === "done"
      ? profiles.names.length === 0
        ? "No profiles in ~/.aws/credentials or ~/.aws/config"
        : `${profiles.names.length} profile${profiles.names.length === 1 ? "" : "s"} on this machine`
      : profiles.state === "failed"
        ? profiles.message
        : "Empty: default. Keys or credential_process; for SSO, a credential_process running aws configure export-credentials";

  return (
    <>
      <Row label="Credentials" htmlFor={`${uid}-auth`}>
        <Controller
          control={control}
          name="driverParams.auth"
          render={({ field }) => (
            <Select
              value={typeof field.value === "string" ? field.value : "accessKey"}
              onValueChange={(v) => {
                field.onChange(v);
                // The Command source reads the secret's Command Line.
                if (v === "command")
                  setValue("passwordMode", "command", {
                    shouldDirty: true,
                    shouldValidate: formState.isSubmitted,
                  });
              }}
            >
              <SelectTrigger id={`${uid}-auth`} className="w-72" aria-label="Credentials">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {AUTH_SOURCES.map(([id, label]) => (
                  <SelectItem key={id} value={id}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {usesKeyId(params.auth) && (
        <UserRow form={form} label="Access key ID" placeholder="AKIAIOSFODNN7EXAMPLE" />
      )}
      {usesSecret(params.auth) && (
        <SecretRow
          form={form}
          hasStoredPassword={hasStoredPassword}
          label="Secret Access Key"
          passwordModes={params.auth === "command" ? ["command"] : passwordModes}
          commandExample="aws configure export-credentials --profile dev --format process"
        />
      )}
      {params.auth === "accessKey" && (
        <Row label="">
          <p className="text-muted-foreground text-[11px]">
            Temporary keys with a session token: use an AWS profile or the Command source.
          </p>
        </Row>
      )}
      {params.auth === "profile" && (
        <>
          {text("profileName", "Profile", "default", profileHint, `${uid}-profiles`)}
          <datalist id={`${uid}-profiles`} data-testid="dynamo-profiles">
            {profiles.state === "done" && profiles.names.map((n) => <option key={n} value={n} />)}
          </datalist>
        </>
      )}
      <Row
        label="Region"
        htmlFor={`${uid}-region`}
        error={formState.errors.database?.message}
        hint={
          params.auth === "local"
            ? "Any name: DynamoDB Local keeps a database per region"
            : undefined
        }
      >
        <Input
          id={`${uid}-region`}
          {...register("database")}
          list={`${uid}-regions`}
          placeholder="us-east-1"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
        <datalist id={`${uid}-regions`} data-testid="dynamo-regions">
          {regions.map((r) => (
            <option key={r} value={r} />
          ))}
        </datalist>
      </Row>
      {text(
        "endpoint",
        "Endpoint",
        params.auth === "local" ? LOCAL_ENDPOINT : "https://dynamodb.<region>.amazonaws.com",
        params.auth === "local"
          ? "Empty: DynamoDB Local on this machine"
          : "Optional: DynamoDB Local, LocalStack, a VPC or FIPS endpoint",
      )}
      {formTlsApplies(params) && (
        <SslRows
          form={form}
          modes={DYNAMO_SSL_MODES}
          hint="AWS's own endpoints are always verified; this applies to this endpoint"
        />
      )}
      <Row label="">
        <div className="flex flex-col">
          {check("consistentReads", "Strongly consistent reads (twice the read capacity)")}
          {check("consumedCapacity", "Ask for the capacity each request consumes")}
        </div>
      </Row>
      {text(
        "sampleSize",
        "Sample size",
        String(DEFAULT_SAMPLE_SIZE),
        "Items read to find a table's attributes",
      )}
      {text(
        "pageRequests",
        "Requests per page",
        String(DEFAULT_PAGE_REQUESTS),
        "A filtered page stops after this many requests, with a cursor to the next",
      )}
    </>
  );
}
