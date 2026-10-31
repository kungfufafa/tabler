import { useId, useState } from "react";
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
import { isWindows } from "@/lib/platform";
import type { KindFieldsProps } from "../types";
import { AUTH_METHODS, instanceHint, mssqlParamsOf, windowsOnly } from "./params";
import { parseConnectionString } from "./url";

/**
 * SQL Server's own rows: a connection string to fill the form from (ADO.NET, JDBC or a URL), the
 * login, a named instance, TDS 8.0 strict encryption, the certificate's name, the read-only
 * application intent, the application name, multi-subnet failover and the packet size.
 */
export function MssqlFields({ form }: KindFieldsProps) {
  const { register, control, formState, watch, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const auth = mssqlParamsOf(watch("driverParams")).auth;
  const [pasted, setPasted] = useState("");
  const [pasteError, setPasteError] = useState<string | null>(null);

  // A pasted string is spread over the form and not kept: its password goes to Password.
  const takeString = (text: string) => {
    if (text.trim() === "") return setPasteError(null);
    const read = parseConnectionString(text);
    if (!read) return setPasteError("Not an ADO.NET, JDBC or sqlserver:// connection string");
    const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };
    for (const [field, value] of Object.entries(read.values))
      setValue(field as Path<ConnectionFormValues>, value as never, opts);
    if (read.password !== null) setValue("password", read.password, opts);
    setPasted("");
    setPasteError(null);
  };

  const check = (key: string, text: string) => (
    <Row label="">
      <label className="flex h-8 items-center gap-2 text-xs">
        <Controller
          control={control}
          name={`driverParams.${key}` as Path<ConnectionFormValues>}
          render={({ field }) => (
            <Checkbox
              checked={field.value === true}
              onCheckedChange={(v) => field.onChange(v === true)}
              aria-label={text}
            />
          )}
        />
        {text}
      </label>
    </Row>
  );

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

  const authHint =
    windowsOnly(auth) && !isWindows
      ? "Works in the Windows build of Tabler only"
      : auth === "integrated"
        ? "Signs in as the Windows user running Tabler: no user or password"
        : auth === "entraToken"
          ? "The password is the token: Command Line runs e.g. az account get-access-token --resource https://database.windows.net/ --query accessToken -o tsv"
          : undefined;

  return (
    <>
      <Row
        label="Authentication"
        htmlFor={`${uid}-auth`}
        error={errors?.auth?.message}
        hint={authHint}
      >
        <Controller
          control={control}
          name="driverParams.auth"
          render={({ field }) => (
            <Select
              value={mssqlParamsOf({ auth: field.value }).auth}
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
        "instance",
        "Instance",
        "SQLEXPRESS",
        instanceHint(watch("sshEnabled"), watch("port")),
      )}
      {check("strict", "Strict encryption (TDS 8.0, SQL Server 2022 and later)")}
      {textRow(
        "hostnameInCertificate",
        "Cert. name",
        "",
        "Optional: the name the server's certificate carries, when it is not the host's",
      )}
      {check("readOnlyIntent", "Read-only application intent (to a readable secondary)")}
      {textRow("applicationName", "App name", "Tabler")}
      {check("multiSubnetFailover", "Multi-subnet failover")}
      {textRow("packetSize", "Packet size", "the server's", "Optional: bytes, 512 to 32767")}
      <Row
        label="Import"
        htmlFor={`${uid}-import`}
        error={pasteError ?? undefined}
        hint="Paste an ADO.NET, JDBC or sqlserver:// string to fill the form"
      >
        <Input
          id={`${uid}-import`}
          value={pasted}
          onChange={(e) => setPasted(e.target.value)}
          onBlur={(e) => takeString(e.target.value)}
          placeholder="Server=tcp:db.example.com,1433;Database=shop;User ID=app"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
          aria-label="Connection string"
        />
      </Row>
    </>
  );
}
