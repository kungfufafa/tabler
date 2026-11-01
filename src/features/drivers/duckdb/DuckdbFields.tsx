import { FilePlus2, Loader2 } from "lucide-react";
import { useState } from "react";
import { Controller, type Path } from "react-hook-form";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Row } from "@/features/connections/rows/Row";
import type { ConnectionFormValues } from "@/features/connections/schema";
import { toIpcError } from "@/lib/ipc";
import type { KindFieldsProps } from "../types";
import { createDuckdbFile, pickNewDuckdbFile } from "./newFile";
import { MEMORY } from "./params";

/**
 * DuckDB's own rows after the file and the key: New… (an empty database file, which the form
 * then opens), `:memory:` for a scratch database, and the autoload switch, which lets DuckDB
 * download the extensions a statement needs (never in a read-only or MCP session).
 */
export function DuckdbFields({ form }: KindFieldsProps) {
  const { control, getValues, setValue, watch } = form;
  const [creating, setCreating] = useState(false);
  const readOnly = watch("readOnly");

  const create = async () => {
    const path = (await pickNewDuckdbFile())?.trim();
    if (!path) return;
    setCreating(true);
    try {
      const made = await createDuckdbFile(getValues(), path);
      setValue("sqlitePath", made, { shouldValidate: true, shouldDirty: true });
    } catch (err) {
      toast.error("Could not create the DuckDB file", { description: toIpcError(err).message });
    } finally {
      setCreating(false);
    }
  };

  return (
    <>
      <Row
        label=""
        hint={
          readOnly
            ? "Turn off Read-only to create a file"
            : `New… creates an empty database file; ${MEMORY} opens a database that lives as long as the connection`
        }
      >
        <div className="flex gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={creating || readOnly}
            title="Create a new, empty DuckDB database file"
            onClick={() => void create()}
          >
            {creating ? <Loader2 className="animate-spin" /> : <FilePlus2 />} New…
          </Button>
          <Button
            type="button"
            variant="outline"
            title="A database in memory, gone when the connection closes"
            onClick={() =>
              setValue("sqlitePath", MEMORY, { shouldValidate: true, shouldDirty: true })
            }
          >
            In memory
          </Button>
        </div>
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
