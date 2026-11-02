import { Controller, type Path } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { KindFieldsProps } from "../types";
import { MEMORY } from "./params";

/**
 * DuckDB's own rows after the file and the key (whose row has New…, `KindUi.newFile`): `:memory:`
 * for a scratch database, and the autoload switch, which lets DuckDB download the extensions a
 * statement needs (never in a read-only or MCP session).
 */
export function DuckdbFields({ form }: KindFieldsProps) {
  const { control, setValue } = form;

  return (
    <>
      <Row label="" hint={`${MEMORY} opens a database that lives as long as the connection`}>
        <Button
          type="button"
          variant="outline"
          className="w-fit"
          title="A database in memory, gone when the connection closes"
          onClick={() =>
            setValue("sqlitePath", MEMORY, { shouldValidate: true, shouldDirty: true })
          }
        >
          In memory
        </Button>
      </Row>
      <Row
        label=""
        hint="Downloads the extensions a statement needs from extensions.duckdb.org into ~/.duckdb. Off: the built-in JSON and Parquet, and what the startup SQL loads"
      >
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name={"driverParams.autoloadExtensions" as Path<ConnectionFormValues>}
            render={({ field }) => (
              <Checkbox
                checked={field.value === true}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Autoload extensions"
              />
            )}
          />
          Autoload extensions
        </label>
      </Row>
    </>
  );
}
