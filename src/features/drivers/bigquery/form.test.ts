import { describe, expect, it } from "vitest";
import {
  connectionDetail,
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import lane from ".";

// The BigQuery form: no host, port, user, SSH or SSL; the project as the database; the secret
// only for an access token; the params saved as `BigqueryParams` and read back; its checks next
// to the schema's, in `BigqueryParams::validate`'s words.

const ui = lane.kinds!.bigquery!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "bigquery",
  database: "demo-proj",
  ...values,
});

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

describe("the BigQuery form", () => {
  it("has only the lane's rows and starts from the backend's defaults", () => {
    expect(ui.rows).toEqual([]);
    expect(ui.labels).toEqual({ database: "Project", secret: "Access token" });
    expect(ui.sslModes).toBeNull();
    expect(ui.defaults!()).toMatchObject({
      host: "",
      port: "",
      passwordMode: "none",
      driverParams: { auth: "serviceAccount", maximumBytesBilled: "1 GB", useQueryCache: true },
    });
  });

  it("saves a key-file profile without a secret, host or TLS files, and reads it back", () => {
    const values = form({
      host: "leftover",
      port: "5432",
      user: "me",
      sslKeyPath: "/k.pem",
      passwordMode: "keychain",
      password: "typed",
      driverParams: params({ keyFilePath: "/keys/sa.json", defaultDataset: "shop" }),
    });
    const { profile } = formToProfileInput(values, null, "id-1", ui);
    expect(profile).toMatchObject({
      driver: "bigquery",
      host: null,
      port: null,
      user: null,
      ssh: null,
      database: "demo-proj",
    });
    expect(profile.options).toMatchObject({
      passwordMode: "none",
      passwordCommand: null,
      sslKeyPath: null,
      sslCertPath: null,
      driverParams: {
        auth: "serviceAccount",
        keyFilePath: "/keys/sa.json",
        defaultDataset: "shop",
        maximumBytesBilled: 2 ** 30,
      },
    });
    const back = profileToForm(profile, ui);
    expect(back.database).toBe("demo-proj");
    expect(back.driverParams).toEqual(
      params({ keyFilePath: "/keys/sa.json", defaultDataset: "shop" }),
    );
    expect(formToProfileInput(back, profile, "id-1", ui).profile.options.driverParams).toEqual(
      profile.options.driverParams,
    );
  });

  it("keeps the token's storage and command for an access token", () => {
    const { profile } = formToProfileInput(
      form({
        passwordMode: "command",
        passwordCommand: "gcloud auth print-access-token",
        driverParams: params({ auth: "accessToken" }),
      }),
      null,
      "id-2",
      ui,
    );
    expect(profile.options).toMatchObject({
      passwordMode: "command",
      passwordCommand: "gcloud auth print-access-token",
      driverParams: { auth: "accessToken", maximumBytesBilled: 2 ** 30 },
    });
  });

  it("says what is missing or malformed, in the backend's words", () => {
    expect(issues(form({ driverParams: params({ keyFilePath: "/k.json" }) }))).toEqual([]);
    expect(
      issues(form({ database: "", driverParams: params({ keyFilePath: "/k.json" }) })),
    ).toEqual(["database: Project is required"]);
    expect(issues(form())).toEqual([
      "driverParams.keyFilePath: Choose the service account's key file (JSON)",
    ]);
    expect(
      issues(
        form({
          driverParams: params({
            auth: "applicationDefault",
            defaultDataset: "my-data",
            maximumBytesBilled: "0",
            labels: "Team = x",
          }),
        }),
      ),
    ).toEqual([
      "driverParams.maximumBytesBilled: Maximum bytes billed must be more than 0 (or empty for no limit)",
      "driverParams.defaultDataset: “my-data” is no dataset name (letters, digits and underscores)",
      "driverParams.labels: The label “Team” must use lower-case letters, digits, _ and - (63 at most, the key starting with a letter)",
    ]);
    expect(issues(form({ driverParams: params({ auth: "accessToken" }) }))).toEqual([
      "passwordMode: An access token is the secret: store it, ask for it, or print it with a command",
    ]);
    expect(
      issues(form({ passwordMode: "command", driverParams: params({ auth: "accessToken" }) })),
    ).toEqual(["passwordCommand: Enter the command that prints the access token"]);
  });

  it("sums a profile up as project · dataset · location · sign-in", () => {
    const { profile } = formToProfileInput(
      form({
        driverParams: params({
          keyFilePath: "/k.json",
          defaultDataset: "shop",
          location: "US",
        }),
      }),
      null,
      "id-3",
      ui,
    );
    expect(ui.summary!(profile)).toBe("demo-proj · shop · US · key file");
    expect(connectionDetail(profile, ui)).toBe("demo-proj · shop · US · key file");
  });
});
