import { useId, type ReactNode } from "react";
import { Controller } from "react-hook-form";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { SslMode } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { baseName, pickFile } from "./files";
import { Row, type ConnectionForm } from "./Row";

type SslFileField = "sslKeyPath" | "sslCertPath" | "sslCaPath";

/** TablePlus's "Key…" / "Cert…" / "CA Cert…" buttons: the chosen file's name, and "—" to clear. */
function SslFileButton({
  form,
  field,
  label,
  title,
}: {
  form: ConnectionForm;
  field: SslFileField;
  label: string;
  title: string;
}) {
  const path = form.watch(field);
  const set = (v: string) =>
    form.setValue(field, v, { shouldDirty: true, shouldValidate: form.formState.isSubmitted });
  return (
    <div className="flex items-center">
      <Button
        type="button"
        variant="outline"
        size="sm"
        title={path ? path : title}
        aria-label={path ? `${label} ${baseName(path)}` : label}
        className={cn("max-w-40", path && "rounded-r-none font-mono text-[11px]")}
        onClick={async () => {
          const p = await pickFile(title);
          if (p) set(p);
        }}
      >
        <span className="truncate">{path ? baseName(path) : label}</span>
      </Button>
      {path && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-label={`Clear ${label.replace("…", "")}`}
          title="Remove"
          className="-ml-px rounded-l-none px-2"
          onClick={() => set("")}
        >
          —
        </Button>
      )}
    </div>
  );
}

/**
 * The SSL mode, in the driver's own words (`modes`, as (mode, label) in menu order), then the
 * key, certificate and CA files once TLS is on or a file is chosen. `extra` follows the mode on
 * its line (PostgreSQL's negotiation).
 */
export function SslRows({
  form,
  modes,
  hint,
  extra,
  keysHint = "PEM files: an unencrypted key (PKCS#8, PKCS#1 or SEC1) and the client certificate followed by its intermediates (one file with both works for Key… and Cert…); the CA certificate for verify-ca / verify-full",
}: {
  form: ConnectionForm;
  modes: ReadonlyArray<readonly [SslMode, string]>;
  hint?: ReactNode;
  extra?: ReactNode;
  keysHint?: string;
}) {
  const { control, formState, watch } = form;
  const errors = formState.errors;
  const uid = useId();
  const ssl = watch("ssl");
  const keys =
    ssl !== "disable" ||
    [watch("sslKeyPath"), watch("sslCertPath"), watch("sslCaPath")].some((p) => p.trim() !== "");
  return (
    <>
      <Row
        label="SSL mode"
        htmlFor={`${uid}-ssl`}
        error={errors.pgSslNegotiation?.message}
        hint={hint}
      >
        <div className="flex items-center gap-2">
          <Controller
            control={control}
            name="ssl"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger id={`${uid}-ssl`} className="w-48">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {modes.map(([m, label]) => (
                    <SelectItem key={m} value={m}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {extra}
        </div>
      </Row>
      {keys && (
        <Row
          label="SSL keys"
          error={errors.sslKeyPath?.message ?? errors.sslCertPath?.message}
          hint={keysHint}
        >
          <div
            role="group"
            aria-label="SSL keys"
            className="flex min-h-8 flex-wrap items-center gap-1.5"
          >
            <SslFileButton
              form={form}
              field="sslKeyPath"
              label="Key…"
              title="Choose the client private key (PEM)"
            />
            <SslFileButton
              form={form}
              field="sslCertPath"
              label="Cert…"
              title="Choose the client certificate (PEM)"
            />
            <SslFileButton
              form={form}
              field="sslCaPath"
              label="CA Cert…"
              title="Choose the CA certificate (PEM)"
            />
          </div>
        </Row>
      )}
    </>
  );
}
