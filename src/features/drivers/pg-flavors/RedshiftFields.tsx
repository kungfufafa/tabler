import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Row } from "@/features/connections/rows/Row";
import type { KindFieldsProps } from "../types";
import { iamSignIn } from "./iam";

/**
 * Redshift's row after the PostgreSQL form: "Use IAM", which sets the password to the AWS CLI
 * command that prints a temporary one (Command mode) and, on a provisioned cluster, the user to
 * the `IAM:` user that password is for.
 */
export function RedshiftFields({ form }: KindFieldsProps) {
  const { getValues, setValue, formState } = form;
  const [note, setNote] = useState<string | null>(null);

  const useIam = () => {
    const v = getValues();
    const signIn = iamSignIn(v.host, v.user, v.database);
    if (typeof signIn === "string") return setNote(signIn);
    const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };
    setValue("passwordMode", "command", opts);
    setValue("passwordCommand", signIn.command, opts);
    if (signIn.user !== null) setValue("user", signIn.user, opts);
    setNote(
      signIn.user === null
        ? "Sign in as your IAM identity: IAM:<user> or IAMR:<role>"
        : "The AWS CLI prints a password for the IAM: user on every connect",
    );
  };

  return (
    <Row label="IAM" hint={note ?? undefined}>
      <div className="flex h-8 items-center">
        <Button type="button" variant="outline" size="sm" onClick={useIam}>
          Use IAM credentials
        </Button>
      </div>
    </Row>
  );
}
