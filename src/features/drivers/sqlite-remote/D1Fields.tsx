import { useId, useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Row } from "@/features/connections/rows/Row";
import { formToProfileInput, type ConnectionFormValues } from "@/features/connections/schema";
import { ipc, toIpcError, type ProfileInput } from "@/lib/ipc";
import type { KindFieldsProps } from "../types";
import { d1ToProfile } from "./params";

/** One database of the account, as `listDatabases` answers it. */
interface D1Database {
  uuid: string;
  name: string;
  createdAt: string | null;
  fileSize: number | null;
}

/** No saved profile's id reaches the form: a stored token is not used for the list. */
const UNSAVED = "00000000-0000-0000-0000-000000000000";

/** The profile and secret the form would connect with, for `listDatabases`. */
function pickerInput(v: ConnectionFormValues): ProfileInput {
  const input = formToProfileInput(v, null, UNSAVED);
  input.profile = d1ToProfile(v, input.profile);
  // "Ask every time" sends the typed token with the call rather than storing it.
  if (v.passwordMode === "ask" && v.password !== "") input.password = v.password;
  return input;
}

/** D1's own rows: the account ID, the database with the account's list, and the API base. */
export function D1Fields({ form, hasStoredPassword }: KindFieldsProps) {
  const { register, formState, getValues, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const [listing, setListing] = useState<
    | { state: "idle" }
    | { state: "loading" }
    | { state: "done"; databases: D1Database[] }
    | { state: "failed"; message: string }
  >({ state: "idle" });

  const list = async () => {
    const values = getValues();
    if (values.passwordMode !== "command" && values.password === "") {
      setListing({
        state: "failed",
        message: hasStoredPassword
          ? "Type the API token again to list the databases (the stored one is used to connect)"
          : "Enter the API token first",
      });
      return;
    }
    setListing({ state: "loading" });
    try {
      const databases = await ipc.driverUtil<D1Database[]>(pickerInput(values), "listDatabases");
      setListing({ state: "done", databases });
      const only = databases.length === 1 ? databases[0] : undefined;
      if (only && values.database === "")
        setValue("database", only.name, {
          shouldDirty: true,
          shouldValidate: formState.isSubmitted,
        });
    } catch (err) {
      setListing({ state: "failed", message: toIpcError(err).message });
    }
  };

  const status =
    listing.state === "done"
      ? listing.databases.length === 0
        ? "The account has no D1 databases"
        : `${listing.databases.length} database${listing.databases.length === 1 ? "" : "s"}: pick one in Database`
      : listing.state === "failed"
        ? listing.message
        : "The database's name or UUID";

  return (
    <>
      <Row
        label="Account ID"
        htmlFor={`${uid}-account`}
        error={errors?.accountId?.message}
        hint="Workers & Pages ▸ Overview"
      >
        <Input
          id={`${uid}-account`}
          {...register("driverParams.accountId")}
          placeholder="0123456789abcdef0123456789abcdef"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      <Row
        label="Database"
        htmlFor={`${uid}-database`}
        error={formState.errors.database?.message}
        hint={<span role="status">{status}</span>}
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-database`}
            {...register("database")}
            list={`${uid}-databases`}
            placeholder="production-db"
            className="font-mono"
            autoComplete="off"
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={listing.state === "loading"}
            onClick={() => void list()}
          >
            {listing.state === "loading" && <Loader2 className="size-3.5 animate-spin" />}
            List databases
          </Button>
        </div>
        <datalist id={`${uid}-databases`} data-testid="d1-databases">
          {listing.state === "done" &&
            listing.databases.map((d) => (
              <option key={d.uuid} value={d.name}>
                {d.uuid}
              </option>
            ))}
        </datalist>
      </Row>
      <Row
        label="API base"
        htmlFor={`${uid}-api`}
        error={errors?.apiBase?.message}
        hint="Optional: a gateway in place of Cloudflare's API"
      >
        <Input
          id={`${uid}-api`}
          {...register("driverParams.apiBase")}
          placeholder="https://api.cloudflare.com/client/v4"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
    </>
  );
}
