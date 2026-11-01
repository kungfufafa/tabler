import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { planOpen } from "@/features/deeplink/route";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { awaitKindUi, registerLaneOverride } from "@/features/drivers/registry";
import type { ConnectionProfile } from "@/lib/ipc";
import { useAppInfoStore } from "@/stores/appInfo";
import lane from ".";
import { formatBigqueryUrl, importBigqueryUrl, parseBigqueryUrl } from "./url";

// `bigquery://` URLs as the real-server suite writes them (`tests/bigquery.rs`), read into the
// form and written back from a profile: a key file, a token (into the secret, never written
// back), ADC, and every setting, round-tripping through the form's Import box. A link (`url.parse`)
// never signs in with ADC or a key file.

const ui = lane.kinds!.bigquery!;

/** The profile the form would save from a URL pasted into its Import box. */
function profileOf(url: string): ConnectionProfile {
  const parsed = importBigqueryUrl(url);
  if (!parsed) throw new Error(url);
  const values: ConnectionFormValues = {
    ...DEFAULT_FORM,
    ...ui.defaults!(),
    name: "n",
    driver: "bigquery",
    ...parsed.values,
    passwordMode: parsed.password ? "keychain" : "none",
    password: parsed.password ?? "",
  } as ConnectionFormValues;
  return formToProfileInput(values, null, "id", ui).profile;
}

describe("bigquery:// URLs", () => {
  it("are the lane's one scheme", () => {
    expect(ui.url!.schemes).toEqual(["bigquery"]);
    expect(ui.url!.parse).toBe(parseBigqueryUrl);
    expect(ui.url!.format).toBe(formatBigqueryUrl);
  });

  it("read a key file and a dataset in the Import box", () => {
    const p = importBigqueryUrl("bigquery://demo-proj?keyFile=/keys/sa.json&dataset=shop")!;
    expect(p.password).toBeNull();
    expect(p.values).toMatchObject({
      host: "",
      port: "",
      database: "demo-proj",
      name: "demo-proj",
    });
    expect(p.values.driverParams).toMatchObject({
      auth: "serviceAccount",
      keyFilePath: "/keys/sa.json",
      defaultDataset: "shop",
      maximumBytesBilled: "1 GB",
      useQueryCache: true,
    });
  });

  it("read a token into the secret, and the dataset from the path", () => {
    const p = parseBigqueryUrl("bigquery://:ya29.a0Af%2Fx@demo-proj/analytics")!;
    expect(p.password).toBe("ya29.a0Af/x");
    expect(p.values.driverParams).toMatchObject({
      auth: "accessToken",
      defaultDataset: "analytics",
    });
  });

  it("read ADC in the Import box when there is neither, and a domain-scoped project as written", () => {
    const p = importBigqueryUrl("bigquery://example.com%3Aanalytics")!;
    expect(p.values.database).toBe("example.com:analytics");
    expect(p.values.driverParams).toMatchObject({ auth: "applicationDefault" });
    expect(importBigqueryUrl("bigquery://p?auth=adc")!.values.driverParams).toMatchObject({
      auth: "applicationDefault",
    });
    expect(parseBigqueryUrl("bigquery://example.com%3Aanalytics")!.values.database).toBe(
      "example.com:analytics",
    );
  });

  // A clicked link would sign the user's own Google identity in to a project it names, and read
  // a key file it chose.
  it("as a link, never sign in with ADC or a key file: an access token instead", () => {
    for (const url of [
      "bigquery://attacker-proj",
      "bigquery://attacker-proj?auth=adc",
      "bigquery://attacker-proj?auth=application_default",
      "bigquery://attacker-proj?auth=serviceAccount",
      "bigquery://attacker-proj?keyFile=/keys/sa.json",
      "bigquery://attacker-proj?keyFilePath=C%3A%5Ckeys%5Csa.json&auth=serviceAccount",
      "bigquery://attacker-proj?auth=adc&endpoint=https://bigquery.googleapis.com/bigquery/v2",
    ]) {
      const p = parseBigqueryUrl(url)!;
      expect(p.password, url).toBeNull();
      expect(p.values.driverParams, url).toMatchObject({ auth: "accessToken", keyFilePath: "" });
    }
    const own = parseBigqueryUrl("bigquery://:t@p?auth=adc&keyFile=/k.json")!;
    expect(own.password).toBe("t");
    expect(own.values.driverParams).toMatchObject({ auth: "accessToken", keyFilePath: "" });
  });

  it("read every setting, and refuse values that cannot be read", () => {
    const p = parseBigqueryUrl(
      "bigquery://p?auth=token&location=EU&endpoint=http://localhost:9050&extraProjects=a,b&maximumBytesBilled=none&useQueryCache=false&jobTimeoutMs=30000&labels=team:data,env:dev",
    )!;
    expect(p.values.driverParams).toMatchObject({
      auth: "accessToken",
      location: "EU",
      apiEndpoint: "http://localhost:9050",
      extraProjects: "a, b",
      maximumBytesBilled: "1 GB",
      useQueryCache: false,
      jobTimeoutSecs: "30",
      labels: "team = data\nenv = dev",
    });
    expect(
      parseBigqueryUrl("bigquery://p?maximumBytesBilled=500MB")!.values.driverParams,
    ).toMatchObject({ maximumBytesBilled: "500 MB" });
    for (const bad of [
      "bigquery://",
      "bigquery:///shop",
      "postgres://h/db",
      "bigquery://p?auth=kerberos",
      "bigquery://p?useQueryCache=maybe",
      "bigquery://p?maximumBytesBilled=0",
      "bigquery://p?maximumBytesBilled=lots",
      "bigquery://p?jobTimeoutMs=1.5",
      "bigquery://p?labels=nocolon",
    ])
      expect(parseBigqueryUrl(bad), bad).toBeNull();
  });

  // Reading a key file on a share makes Windows sign in to its host (NTLM) before any check.
  it("never read a key file from a network share", () => {
    for (const bad of [
      "bigquery://p?keyFile=%5C%5Cattacker.example%5Cs%5Ck.json",
      "bigquery://p?keyFile=//attacker.example/s/k.json",
      "bigquery://p?keyFile=%2F%2Fattacker.example%2Fs%2Fk.json",
      "bigquery://p?keyFilePath=%5C%5C%3F%5CUNC%5Cattacker.example%5Cs%5Ck.json",
      "bigquery://p?keyFile=%5C%5C.%5Cpipe%5Cx",
      "bigquery://p?keyFile=%5C%3F%3F%5CUNC%5Cattacker.example%5Cs%5Ck.json",
      "bigquery://p?keyFile=k.json",
    ]) {
      expect(parseBigqueryUrl(bad), bad).toBeNull();
      expect(importBigqueryUrl(bad), bad).toBeNull();
    }
    for (const ok of ["/keys/sa.json", "C:\\keys\\sa.json", "C:/keys/sa.json"])
      expect(
        importBigqueryUrl(`bigquery://p?keyFile=${encodeURIComponent(ok)}`)!.values.driverParams,
        ok,
      ).toMatchObject({ auth: "serviceAccount", keyFilePath: ok });
  });

  // A clicked tabler:// link connects with what parse returns, unasked.
  it("never send ambient credentials to an endpoint that is not Google's or this machine's", () => {
    for (const bad of [
      "bigquery://victim-proj?auth=adc&endpoint=https://evil.example/bigquery/v2",
      "bigquery://victim-proj?endpoint=https%3A%2F%2Fevil.example%2Fbigquery%2Fv2",
      "bigquery://victim-proj?keyFile=/keys/sa.json&apiEndpoint=https://evil.example",
      "bigquery://victim-proj?auth=token&endpoint=https://evil.example",
      "bigquery://p?auth=adc&endpoint=https://googleapis.com.evil.example",
      "bigquery://p?auth=adc&endpoint=http://bigquery.googleapis.com/bigquery/v2",
      "bigquery://p?auth=adc&endpoint=https://evil.example%2F@bigquery.googleapis.com",
      "bigquery://p?auth=adc&endpoint=http://localhost.evil.example",
      "bigquery://:t@p?endpoint=https://bq.example.com/v2%3Fx%3D",
      "bigquery://:t@p?endpoint=https://bq.example.com/v2%23x",
    ]) {
      expect(parseBigqueryUrl(bad), bad).toBeNull();
      expect(importBigqueryUrl(bad), bad).toBeNull();
    }
    const cases: [string, string][] = [
      ["bigquery://p?auth=adc&endpoint=http://127.0.0.1:9050", "http://127.0.0.1:9050"],
      [
        "bigquery://p?keyFile=/k.json&endpoint=https://bigquery-psc.p.googleapis.com/bigquery/v2",
        "https://bigquery-psc.p.googleapis.com/bigquery/v2",
      ],
      // The link's own token: harmless to send where the link says.
      ["bigquery://:their-token@p?endpoint=https://bq.example.com", "https://bq.example.com"],
    ];
    for (const [url, endpoint] of cases)
      expect(importBigqueryUrl(url)!.values.driverParams, url).toMatchObject({
        apiEndpoint: endpoint,
      });
  });

  it("tighten the cost guard, never loosen or remove it", () => {
    for (const url of [
      "bigquery://p?auth=adc&maximumBytesBilled=none",
      "bigquery://p?maximumBytesBilled=NONE",
      "bigquery://p?maximumBytesBilled=10TB",
      "bigquery://p?maximumBytesBilled=1GB",
    ])
      expect(parseBigqueryUrl(url)!.values.driverParams, url).toMatchObject({
        maximumBytesBilled: "1 GB",
      });
    expect(
      parseBigqueryUrl("bigquery://p?maximumBytesBilled=100MB")!.values.driverParams,
    ).toMatchObject({ maximumBytesBilled: "100 MB" });
  });

  it("write a profile's project and settings back, never its token", () => {
    const url =
      "bigquery://demo-proj?auth=serviceAccount&keyFile=%2Fkeys%2Fsa.json&dataset=shop&location=US&extraProjects=bigquery-public-data&maximumBytesBilled=524288000&useQueryCache=false&jobTimeoutMs=90000&labels=team%3Adata&endpoint=https%3A%2F%2Fbigquery-psc.p.googleapis.com%2Fbigquery%2Fv2";
    expect(formatBigqueryUrl(profileOf(url))).toBe(url);
    const token = profileOf("bigquery://:secret-token@demo-proj");
    expect(formatBigqueryUrl(token)).toBe("bigquery://demo-proj?auth=accessToken");
    expect(formatBigqueryUrl({ ...token, database: null })).toBeNull();
  });

  it("leave out what parse would not take back: no guard, a larger one, an untrusted endpoint", () => {
    const base = profileOf("bigquery://p?auth=adc");
    const withParams = (extra: Record<string, unknown>): ConnectionProfile =>
      ({
        ...base,
        options: { ...base.options, driverParams: { ...base.options.driverParams, ...extra } },
      }) as ConnectionProfile;
    for (const extra of [
      { maximumBytesBilled: null },
      { maximumBytesBilled: 10 * 2 ** 40 },
      { apiEndpoint: "https://bq.example.com" },
    ])
      expect(formatBigqueryUrl(withParams(extra)), JSON.stringify(extra)).toBe(
        "bigquery://p?auth=applicationDefault",
      );
    const token = profileOf("bigquery://:t@p?endpoint=https://bq.example.com");
    expect(formatBigqueryUrl(token)).toBe("bigquery://p?auth=accessToken");
  });

  it("round-trip: format(parse(format(p))) is format(p)", () => {
    for (const url of [
      "bigquery://demo-proj?keyFile=/k.json",
      "bigquery://:t@demo-proj/analytics?location=asia-southeast2",
      "bigquery://example.com%3Aproj?auth=adc&labels=a:b&maximumBytesBilled=200MB",
      "bigquery://p?auth=adc&maximumBytesBilled=none&endpoint=http://localhost:9050",
      "bigquery://:t@p?endpoint=https://bq.example.com&maximumBytesBilled=5TB",
    ]) {
      const once = formatBigqueryUrl(profileOf(url))!;
      expect(formatBigqueryUrl(profileOf(once)), url).toBe(once);
    }
  });
});

describe("a tabler:// link to a bigquery:// URL", () => {
  afterEach(() => {
    registerLaneOverride("bigquery", null);
    useAppInfoStore.setState({ info: null });
  });

  async function plan(url: string) {
    useAppInfoStore.setState({
      info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, "bigquery"] },
    });
    registerLaneOverride("bigquery", lane);
    await awaitKindUi("bigquery");
    const link = `tabler://open?url=${encodeURIComponent(url)}`;
    return planOpen(
      { kind: "url", url: link },
      { profiles: [], temporary: new Set(), activeConnectionId: null },
    );
  }

  it("opens no connection that sends ambient credentials elsewhere, or lifts the guard", async () => {
    const evil = await plan(
      "bigquery://victim-proj?auth=adc&endpoint=https://evil.example/bigquery/v2&maximumBytesBilled=none",
    );
    expect(evil.kind).toBe("error");
    const own = await plan("bigquery://victim-proj?auth=adc&maximumBytesBilled=none");
    expect(own.kind).toBe("temporary");
    if (own.kind !== "temporary") return;
    expect(own.values.driverParams).toMatchObject({
      auth: "accessToken",
      apiEndpoint: "",
      maximumBytesBilled: "1 GB",
    });
  });

  it("opens no connection that signs in with ADC or a key file the link chose", async () => {
    for (const url of [
      "bigquery://attacker-proj",
      "bigquery://attacker-proj?auth=adc",
      "bigquery://attacker-proj?keyFile=%2Fhome%2Fme%2F.config%2Fgcloud%2Fsa.json",
    ]) {
      const p = await plan(url);
      expect(p.kind, url).toBe("temporary");
      if (p.kind !== "temporary") continue;
      expect(p.password, url).toBeNull();
      expect(p.values.driverParams, url).toMatchObject({ auth: "accessToken", keyFilePath: "" });
    }
  });

  it("opens no connection that reads a key file from a share", async () => {
    for (const keyFile of [
      "\\\\attacker.example\\s\\k.json",
      "//attacker.example/s/k.json",
      "\\\\?\\UNC\\attacker.example\\s\\k.json",
    ])
      expect((await plan(`bigquery://p?keyFile=${encodeURIComponent(keyFile)}`)).kind).toBe(
        "error",
      );
  });
});
