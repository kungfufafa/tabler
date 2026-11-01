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
import {
  cassandraParamsOf,
  COMPRESSIONS,
  CONSISTENCIES,
  PAGE_SIZE,
  REQUEST_TIMEOUT,
  SERIAL_CONSISTENCIES,
} from "./params";

/**
 * Cassandra's own rows: the consistency statements run at (cqlsh's `CONSISTENCY` changes the
 * editor's), the serial consistency of lightweight transactions (the grid's edits), the local
 * datacenter, the request timeout, the editor's page size, compression and single node.
 */
export function CassandraFields({ form }: KindFieldsProps) {
  const { register, control, formState, watch } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const ssh = watch("sshEnabled");

  const select = (
    key: "consistency" | "serialConsistency" | "compression",
    label: string,
    options: readonly (readonly [string, string])[],
    hint?: string,
  ) => (
    <Row label={label} htmlFor={`${uid}-${key}`} error={errors?.[key]?.message} hint={hint}>
      <Controller
        control={control}
        name={`driverParams.${key}` as Path<ConnectionFormValues>}
        render={({ field }) => (
          <Select
            value={cassandraParamsOf({ [key]: field.value })[key]}
            onValueChange={(v) => field.onChange(v)}
          >
            <SelectTrigger id={`${uid}-${key}`} className="w-48" aria-label={label}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {options.map(([id, text]) => (
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

  const levels = (all: readonly string[]) => all.map((c) => [c, c] as const);

  return (
    <>
      {select(
        "consistency",
        "Consistency",
        levels(CONSISTENCIES),
        "What reads and writes wait for; CONSISTENCY in the editor changes it for the editor",
      )}
      {select(
        "serialConsistency",
        "Serial",
        levels(SERIAL_CONSISTENCIES),
        "The serial consistency of lightweight transactions (the grid's edits are ones)",
      )}
      {textRow(
        "localDatacenter",
        "Datacenter",
        "datacenter1",
        "Optional: the datacenter whose nodes to prefer (several datacenters, AWS Keyspaces)",
      )}
      {textRow(
        "requestTimeoutSecs",
        "Timeout",
        String(REQUEST_TIMEOUT.default),
        `Optional: seconds one request may take, ${REQUEST_TIMEOUT.min} to ${REQUEST_TIMEOUT.max}`,
      )}
      {textRow(
        "pageSize",
        "Page size",
        String(PAGE_SIZE.default),
        `Optional: rows per page of the editor's runs, ${PAGE_SIZE.min} to ${PAGE_SIZE.max.toLocaleString("en-US")}`,
      )}
      {select("compression", "Compression", COMPRESSIONS)}
      <Row
        label=""
        hint={
          ssh
            ? "Always on over SSH: the tunnel reaches one node"
            : "Talk to this host only, never to the other nodes it names"
        }
      >
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name="driverParams.singleNode"
            render={({ field }) => (
              <Checkbox
                checked={field.value === true || ssh}
                disabled={ssh}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Single node"
              />
            )}
          />
          Single node
        </label>
      </Row>
    </>
  );
}
