import { useId } from "react";
import { Input } from "@/components/ui/input";
import { Row, type ConnectionForm } from "./Row";

/** The user name to sign in as. */
export function UserRow({
  form,
  label = "User",
  placeholder = "root",
}: {
  form: ConnectionForm;
  label?: string;
  placeholder?: string;
}) {
  const { register, formState } = form;
  const errors = formState.errors;
  const uid = useId();
  return (
    <Row label={label} htmlFor={`${uid}-user`} error={errors.user?.message}>
      <Input
        id={`${uid}-user`}
        {...register("user")}
        placeholder={placeholder}
        className="font-mono"
        autoComplete="off"
        aria-invalid={errors.user ? true : undefined}
      />
    </Row>
  );
}
