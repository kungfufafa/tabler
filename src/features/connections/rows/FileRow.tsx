import { FilePlus2, FolderOpen } from "lucide-react";
import { useId } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { pickFile, type FileFilter } from "./files";
import { Row, type ConnectionForm } from "./Row";

/**
 * The database file of a file-based driver: its path, "Choose…" (the open panel, filtered to the
 * driver's file extensions) and, where the driver can create one, "New…".
 */
export function FileRow({
  form,
  label = "Database file",
  placeholder = "/path/to/database",
  pickerTitle = "Open database",
  filters = [],
  onNew,
  newTitle = "Create a new, empty database file",
}: {
  form: ConnectionForm;
  label?: string;
  placeholder?: string;
  /** The open panel's title. */
  pickerTitle?: string;
  filters?: readonly FileFilter[];
  /** Creates a new file and puts its path in the form; without it there is no "New…". */
  onNew?: () => void | Promise<void>;
  newTitle?: string;
}) {
  const { register, formState, setValue } = form;
  const errors = formState.errors;
  const uid = useId();
  return (
    <Row label={label} htmlFor={`${uid}-file`} error={errors.sqlitePath?.message}>
      <div className="flex gap-2">
        <Input
          id={`${uid}-file`}
          {...register("sqlitePath")}
          placeholder={placeholder}
          className="font-mono"
          aria-invalid={errors.sqlitePath ? true : undefined}
        />
        <Button
          type="button"
          variant="outline"
          title="Open an existing database file"
          onClick={async () => {
            const p = await pickFile(pickerTitle, filters);
            if (p) setValue("sqlitePath", p, { shouldValidate: true, shouldDirty: true });
          }}
        >
          <FolderOpen /> Choose…
        </Button>
        {onNew && (
          <Button type="button" variant="outline" title={newTitle} onClick={() => void onNew()}>
            <FilePlus2 /> New…
          </Button>
        )}
      </div>
    </Row>
  );
}
