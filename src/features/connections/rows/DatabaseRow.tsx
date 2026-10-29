import { useId } from "react";
import { Input } from "@/components/ui/input";
import { Row, type ConnectionForm } from "./Row";

/** The database (or what the driver calls it: "DB index", "Keyspace") to open. */
export function DatabaseRow({
  form,
  label = "Database",
  placeholder,
  hint = "Optional",
}: {
  form: ConnectionForm;
  label?: string;
  placeholder?: string;
  hint?: string;
}) {
  const { register, formState } = form;
  const uid = useId();
  return (
    <Row
      label={label}
      htmlFor={`${uid}-database`}
      error={formState.errors.database?.message}
      hint={hint}
    >
      <Input
        id={`${uid}-database`}
        {...register("database")}
        placeholder={placeholder}
        className="font-mono"
      />
    </Row>
  );
}
