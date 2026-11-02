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
  DEFAULT_PREVIEW_BYTES,
  DEFAULT_PREVIEW_ELEMENTS,
  DEFAULT_SEPARATOR,
  PROTOCOLS,
  redisFormOf,
} from "./params";

/**
 * Redis's own rows: the protocol, the key browser's SCAN count hint (lower it for a slow
 * server), how much of each value the grid previews, whether it reads each key's memory, and the
 * key tree's separator.
 */
export function RedisFields({ form }: KindFieldsProps) {
  const { register, control, formState } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;

  const text = (key: string, label: string, placeholder: string, hint: string) => (
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
      <Row label="Protocol" htmlFor={`${uid}-protocol`} error={errors?.protocol?.message}>
        <Controller
          control={control}
          name={"driverParams.protocol" as Path<ConnectionFormValues>}
          render={({ field }) => (
            <Select
              value={redisFormOf({ protocol: field.value }).protocol}
              onValueChange={(v) => field.onChange(v)}
            >
              <SelectTrigger id={`${uid}-protocol`} className="w-64" aria-label="Protocol">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PROTOCOLS.map(([id, label]) => (
                  <SelectItem key={id} value={id}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {text(
        "scanCount",
        "SCAN count",
        "the page size",
        "Keys each SCAN step asks for; lower it for a slow or busy server",
      )}
      {text(
        "previewBytes",
        "Preview bytes",
        String(DEFAULT_PREVIEW_BYTES),
        "How much of a string the grid reads (GETRANGE)",
      )}
      {text(
        "previewElements",
        "Preview elements",
        String(DEFAULT_PREVIEW_ELEMENTS),
        "Elements of a list, set, hash or stream the grid reads",
      )}
      {text(
        "keySeparator",
        "Key separator",
        DEFAULT_SEPARATOR,
        "What separates the levels of a key name (user:1:name)",
      )}
      <Row label="" hint="MEMORY USAGE of each key in the grid (a sampled estimate)">
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name={"driverParams.memoryColumn" as Path<ConnectionFormValues>}
            render={({ field }) => (
              <Checkbox
                checked={field.value !== false}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Memory column"
              />
            )}
          />
          Memory column
        </label>
      </Row>
    </>
  );
}
