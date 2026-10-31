import { useId } from "react";
import { Controller, useWatch, type Path } from "react-hook-form";
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
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { KindFieldsProps } from "../types";
import { cloudIdUrl, rootOf } from "./address";
import { AUTH_METHODS, AWS_CREDENTIALS, AWS_SERVICES, elasticFormOf } from "./params";

/** What the Password field holds for each sign-in. */
const SECRET_HINT: Record<string, string> = {
  basic: "User and Password sign in (nothing is sent while both are empty)",
  apiKey: "Password holds the API key (encoded, or the key of the API key id)",
  bearer: "Password holds the token (a service-account token, an OAuth or JWT realm's)",
  awsSigv4: "",
  none: "Nothing is sent: a cluster without security",
};

/**
 * The lane's own rows: where the cluster is beyond Host (node URLs, a Cloud ID, a path prefix
 * behind a proxy), the sign-in and its settings (API key id; SigV4's credentials, region and
 * service), the CA fingerprint, system indices and the request time out.
 */
export function ElasticFields({ form }: KindFieldsProps) {
  const { register, control, formState } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const params = elasticFormOf(
    useWatch({ control, name: "driverParams" }) as Record<string, unknown> | undefined,
  );
  const cloud = params.cloudId !== "" ? cloudIdUrl(params.cloudId) : null;

  const select = (
    key: string,
    label: string,
    items: readonly (readonly [string, string])[],
    fallback: string,
    hint?: string,
  ) => {
    const name = `driverParams.${key}` as Path<ConnectionFormValues>;
    return (
      <Row label={label} htmlFor={`${uid}-${key}`} hint={hint} error={errors?.[key]?.message}>
        <Controller
          control={control}
          name={name}
          render={({ field }) => (
            <Select
              // Radix's Select has no empty value: "" (from the host) is shown as "auto".
              value={typeof field.value === "string" && field.value !== "" ? field.value : fallback}
              onValueChange={(v) => field.onChange(v === "auto" ? "" : v)}
            >
              <SelectTrigger id={`${uid}-${key}`} className="w-72" aria-label={label}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {items.map(([id, text]) => (
                  <SelectItem key={id || "auto"} value={id || "auto"}>
                    {text}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
    );
  };

  const text = (key: string, label: string, placeholder: string, hint?: string) => (
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
        label="Node URLs"
        htmlFor={`${uid}-urls`}
        error={errors?.urls?.message}
        hint="Optional, in place of Host: one per line, tried in order at connect"
      >
        <Textarea
          id={`${uid}-urls`}
          {...register("driverParams.urls")}
          placeholder={"https://es1.example.com:9200\nhttps://es2.example.com:9200"}
          className="min-h-14 font-mono text-xs"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      {text(
        "cloudId",
        "Cloud ID",
        "deployment:ZXUtd2VzdC0xLmF3cy5mb3VuZC5pbyQ…",
        cloud
          ? `Connects to ${rootOf(cloud)}`
          : "Optional, in place of Host: an Elastic Cloud deployment",
      )}
      {text(
        "pathPrefix",
        "Path prefix",
        "/es",
        "Behind a reverse proxy that serves the cluster under a path",
      )}
      {select("auth", "Sign-in", AUTH_METHODS, "basic", SECRET_HINT[params.auth] || undefined)}
      {params.auth === "apiKey" &&
        text(
          "apiKeyId",
          "API key id",
          "",
          "With an id, Password holds the key; without, the encoded key",
        )}
      {params.auth === "awsSigv4" && (
        <>
          {select(
            "awsCredentials",
            "AWS keys",
            AWS_CREDENTIALS,
            "accessKey",
            params.awsCredentials === "accessKey"
              ? "User is the access key id, Password the secret access key (or a command's credential_process JSON)"
              : undefined,
          )}
          {params.awsCredentials === "profile" && text("awsProfile", "AWS profile", "default")}
          {text(
            "awsRegion",
            "Region",
            "from the host",
            "Empty: read from the endpoint's host name",
          )}
          {select("awsService", "Service", AWS_SERVICES, "auto")}
        </>
      )}
      {text(
        "caFingerprint",
        "CA fingerprint",
        "SHA-256 in hex",
        "Optional: the cluster's CA certificate pin (ca_trusted_fingerprint), checked in every SSL mode",
      )}
      {text("requestTimeoutSecs", "Request timeout", "seconds (none: only the query timeout)")}
      <Row label="System indices" htmlFor={`${uid}-system`}>
        <Controller
          control={control}
          name={"driverParams.showSystemIndices" as Path<ConnectionFormValues>}
          render={({ field }) => (
            <label className="flex h-8 items-center gap-2 text-xs">
              <Checkbox
                id={`${uid}-system`}
                checked={field.value === true}
                onCheckedChange={(c) => field.onChange(c === true)}
              />
              Show hidden and dot-prefixed indices
            </label>
          )}
        />
      </Row>
    </>
  );
}
