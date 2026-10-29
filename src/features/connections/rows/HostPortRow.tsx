import { useId, type ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Row, type ConnectionForm } from "./Row";

/** Host and Port on one line (a toggle, like MySQL's "Use socket", may follow them). */
export function HostPortRow({
  form,
  label = "Host",
  placeholder = "127.0.0.1",
  defaultPort,
  hint,
  trailing,
}: {
  form: ConnectionForm;
  label?: string;
  placeholder?: string;
  /** The port the driver connects to when none is typed (the field's placeholder). */
  defaultPort: number | null;
  hint?: ReactNode;
  trailing?: ReactNode;
}) {
  const { register, formState } = form;
  const errors = formState.errors;
  const uid = useId();
  return (
    <Row
      label={label}
      htmlFor={`${uid}-host`}
      error={errors.host?.message ?? errors.port?.message}
      hint={hint}
    >
      <div className="flex items-center gap-2">
        <Input
          id={`${uid}-host`}
          {...register("host")}
          placeholder={placeholder}
          className="font-mono"
          aria-invalid={errors.host ? true : undefined}
        />
        <Label
          htmlFor={`${uid}-port`}
          className="text-muted-foreground shrink-0 text-xs font-normal"
        >
          Port
        </Label>
        <Input
          id={`${uid}-port`}
          {...register("port")}
          placeholder={defaultPort === null ? "" : String(defaultPort)}
          className="w-20 shrink-0 font-mono"
          inputMode="numeric"
          aria-invalid={errors.port ? true : undefined}
        />
        {trailing}
      </div>
    </Row>
  );
}
