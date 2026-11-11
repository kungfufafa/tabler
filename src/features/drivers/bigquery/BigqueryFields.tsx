import { FolderOpen } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { Controller, type Path } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { pickFile } from "@/features/connections/rows/files";
import { Row } from "@/features/connections/rows/Row";
import { SecretRow } from "@/features/connections/rows/SecretRow";
import type { ConnectionFormValues } from "@/features/connections/schema";
import { ipc, toIpcError } from "@/lib/ipc";
import type { KindFieldsProps } from "../types";
import { AUTHS, bigqueryParamsOf, DEFAULT_API_ENDPOINT } from "./params";
import {
  type BigqueryDataset,
  type BigqueryProject,
  followAuth,
  type Listing,
  missingForList,
  pickerInput,
  TOKEN_MODES,
} from "./picker";
import { importBigqueryUrl } from "./url";

/**
 * BigQuery's rows: the project with the projects the credentials see, the sign-in (a key file
 * with Choose…, Application Default Credentials, or an access token as the secret), the default
 * dataset with the project's datasets, where jobs run, the extra projects listed as databases,
 * the cost guard (empty: no limit, with a warning), the query cache, the job timeout, job labels,
 * an endpoint of its own (an emulator), and an Import box for a `bigquery://` URL.
 */
export function BigqueryFields({ form, hasStoredPassword }: KindFieldsProps) {
  const { register, control, formState, watch, getValues, setValue } = form;
  const uid = useId();
  const errors = formState.errors.driverParams as Record<string, { message?: string }> | undefined;
  const params = bigqueryParamsOf(watch("driverParams"));
  const [projects, setProjects] = useState<Listing<BigqueryProject>>({ state: "idle" });
  const [datasets, setDatasets] = useState<Listing<BigqueryDataset>>({ state: "idle" });
  const [pasted, setPasted] = useState("");
  const [pasteError, setPasteError] = useState<string | null>(null);

  // A pasted URL is spread over the form and not kept. Unlike a link, it may pick ADC or a key
  // file: the sign-in it picks is in view here. Its token goes to the secret.
  const takeUrl = (text: string) => {
    if (text.trim() === "") return setPasteError(null);
    const read = importBigqueryUrl(text);
    if (!read) return setPasteError("Not a bigquery:// URL this form can read");
    const opts = { shouldDirty: true, shouldValidate: formState.isSubmitted };
    const auth = bigqueryParamsOf(read.values.driverParams).auth;
    setValue("database", read.values.database ?? "", opts);
    setValue("driverParams", read.values.driverParams ?? {}, opts);
    followAuth(form, auth);
    if (read.password !== null) setValue("password", read.password, opts);
    setPasted("");
    setPasteError(null);
  };

  const list = async <T,>(
    method: "listProjects" | "listDatasets",
    set: (l: Listing<T>) => void,
    pick: (items: T[]) => void,
  ) => {
    const values = getValues();
    const missing =
      missingForList(values, hasStoredPassword) ??
      (method === "listDatasets" && values.database.trim() === ""
        ? "Enter the project first"
        : null);
    if (missing) {
      set({ state: "failed", message: missing });
      return;
    }
    set({ state: "loading" });
    try {
      const items = await ipc.driverUtil<T[]>(pickerInput(values), method);
      const list = Array.isArray(items) ? items : [];
      set({ state: "done", items: list });
      pick(list);
    } catch (err) {
      set({ state: "failed", message: toIpcError(err).message });
    }
  };

  const fill = (field: "database" | "driverParams.defaultDataset", value: string) =>
    setValue(field, value, { shouldDirty: true, shouldValidate: formState.isSubmitted });

  const listProjects = () =>
    list<BigqueryProject>("listProjects", setProjects, (items) => {
      const only = items.length === 1 ? items[0] : undefined;
      if (only && getValues("database").trim() === "") fill("database", only.id);
    });
  const listDatasets = () =>
    list<BigqueryDataset>("listDatasets", setDatasets, (items) => {
      const only = items.length === 1 ? items[0] : undefined;
      if (only && bigqueryParamsOf(getValues("driverParams")).defaultDataset === "")
        fill("driverParams.defaultDataset", only.name);
    });

  const status = <T,>(l: Listing<T>, noun: string, fallback: string): ReactNode =>
    l.state === "done"
      ? l.items.length === 0
        ? `No ${noun}s found`
        : `${l.items.length} ${noun}${l.items.length === 1 ? "" : "s"}: pick one`
      : l.state === "failed"
        ? l.message
        : fallback;

  const text = (key: string, label: string, placeholder: string, hint?: ReactNode) => (
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

  const listButton = (label: string, loading: boolean, onClick: () => void) => (
    <Button type="button" variant="outline" size="sm" disabled={loading} onClick={onClick}>
      {label}
    </Button>
  );

  const unlimited = params.maximumBytesBilled.trim() === "";

  return (
    <>
      <Row
        label="Project"
        htmlFor={`${uid}-project`}
        error={formState.errors.database?.message}
        hint={
          <span role="status">
            {status(projects, "project", "The project jobs run and bill in (its ID)")}
          </span>
        }
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-project`}
            {...register("database")}
            list={`${uid}-projects`}
            placeholder="my-project-123"
            className="font-mono"
            autoComplete="off"
            spellCheck={false}
          />
          {listButton("List projects", projects.state === "loading", () => void listProjects())}
        </div>
        <datalist id={`${uid}-projects`} data-testid="bigquery-projects">
          {projects.state === "done" &&
            projects.items.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name ?? p.id}
              </option>
            ))}
        </datalist>
      </Row>
      <Row label="Sign in with" htmlFor={`${uid}-auth`}>
        <Controller
          control={control}
          name={"driverParams.auth" as Path<ConnectionFormValues>}
          render={({ field }) => (
            <Select
              value={typeof field.value === "string" ? field.value : "serviceAccount"}
              onValueChange={(v) => {
                field.onChange(v);
                followAuth(form, v);
              }}
            >
              <SelectTrigger id={`${uid}-auth`} className="w-72" aria-label="Sign in with">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {AUTHS.map(([id, label]) => (
                  <SelectItem key={id} value={id}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {params.auth === "serviceAccount" && (
        <Row
          label="Key file"
          htmlFor={`${uid}-keyFilePath`}
          error={errors?.keyFilePath?.message}
          hint="The service account's JSON key, read when connecting and never stored"
        >
          <div className="flex gap-2">
            <Input
              id={`${uid}-keyFilePath`}
              {...register("driverParams.keyFilePath" as Path<ConnectionFormValues>)}
              placeholder="/path/to/service-account.json"
              className="font-mono"
              autoComplete="off"
              spellCheck={false}
            />
            <Button
              type="button"
              variant="outline"
              title="Choose the service account's key file"
              onClick={async () => {
                const p = await pickFile("Service account key", [
                  { name: "JSON key", extensions: ["json"] },
                ]);
                if (p)
                  setValue("driverParams.keyFilePath" as Path<ConnectionFormValues>, p, {
                    shouldDirty: true,
                    shouldValidate: formState.isSubmitted,
                  });
              }}
            >
              <FolderOpen /> Choose…
            </Button>
          </div>
        </Row>
      )}
      {params.auth === "applicationDefault" && (
        <Row label="">
          <p className="text-muted-foreground text-[11px]">
            GOOGLE_APPLICATION_CREDENTIALS, or the file of gcloud auth application-default login
          </p>
        </Row>
      )}
      {params.auth === "accessToken" && (
        <SecretRow
          form={form}
          hasStoredPassword={hasStoredPassword}
          label="Access token"
          passwordModes={TOKEN_MODES}
          commandExample="gcloud auth print-access-token"
        />
      )}
      <Row
        label="Dataset"
        htmlFor={`${uid}-defaultDataset`}
        error={errors?.defaultDataset?.message}
        hint={
          <span role="status">
            {status(datasets, "dataset", "Optional: where unqualified table names resolve")}
          </span>
        }
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-defaultDataset`}
            {...register("driverParams.defaultDataset" as Path<ConnectionFormValues>)}
            list={`${uid}-datasets`}
            placeholder="analytics"
            className="font-mono"
            autoComplete="off"
            spellCheck={false}
          />
          {listButton("List datasets", datasets.state === "loading", () => void listDatasets())}
        </div>
        <datalist id={`${uid}-datasets`} data-testid="bigquery-datasets">
          {datasets.state === "done" &&
            datasets.items.map((d) => (
              <option key={d.name} value={d.name}>
                {d.location ?? ""}
              </option>
            ))}
        </datalist>
      </Row>
      {text(
        "location",
        "Location",
        "US",
        "Optional: where jobs run (US, EU, asia-southeast2); needed outside the US and EU",
      )}
      {text(
        "extraProjects",
        "More projects",
        "bigquery-public-data",
        "Optional: other projects listed in the sidebar, comma-separated (jobs still run in Project)",
      )}
      <Row
        label="Max billed"
        htmlFor={`${uid}-maximumBytesBilled`}
        error={errors?.maximumBytesBilled?.message}
        hint={
          unlimited ? (
            <span className="text-amber-600 dark:text-amber-500">
              No limit: a query is billed for everything it scans
            </span>
          ) : (
            "Queries that would bill more fail before they run (1 GB, 500 MB or bytes; empty for no limit)"
          )
        }
      >
        <Input
          id={`${uid}-maximumBytesBilled`}
          {...register("driverParams.maximumBytesBilled" as Path<ConnectionFormValues>)}
          placeholder="No limit"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      <Row label="">
        <label className="flex h-8 items-center gap-2 text-xs">
          <Controller
            control={control}
            name={"driverParams.useQueryCache" as Path<ConnectionFormValues>}
            render={({ field }) => (
              <Checkbox
                checked={field.value !== false}
                onCheckedChange={(v) => field.onChange(v === true)}
                aria-label="Use cached results"
              />
            )}
          />
          Use cached results (free when a query repeats)
        </label>
      </Row>
      {text(
        "jobTimeoutSecs",
        "Job timeout",
        "",
        "Optional: seconds after which BigQuery stops a job (empty: its own limit)",
      )}
      <Row
        label="Labels"
        htmlFor={`${uid}-labels`}
        error={errors?.labels?.message}
        hint="Optional: one key = value per line, on every job (for billing reports)"
      >
        <Textarea
          id={`${uid}-labels`}
          {...register("driverParams.labels" as Path<ConnectionFormValues>)}
          placeholder="team = data"
          className="min-h-16 font-mono text-xs"
          autoComplete="off"
          spellCheck={false}
        />
      </Row>
      {text(
        "apiEndpoint",
        "API endpoint",
        DEFAULT_API_ENDPOINT,
        "Optional: an emulator (plain http only on this machine)",
      )}
      <Row
        label="Import"
        htmlFor={`${uid}-import`}
        error={pasteError ?? undefined}
        hint="Paste a bigquery:// URL to fill the form"
      >
        <Input
          id={`${uid}-import`}
          value={pasted}
          onChange={(e) => setPasted(e.target.value)}
          onBlur={(e) => takeUrl(e.target.value)}
          placeholder="bigquery://my-project?keyFile=/keys/sa.json&dataset=analytics"
          className="font-mono"
          autoComplete="off"
          spellCheck={false}
          aria-label="BigQuery URL"
        />
      </Row>
    </>
  );
}
