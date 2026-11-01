import { describe, expect, it } from "vitest";
import {
  bigqueryParamsOf,
  DEFAULT_MAXIMUM_BYTES_BILLED,
  endpointIsBase,
  endpointOk,
  formatBytes,
  googleEndpointOk,
  labelProblem,
  localFilePath,
  paramIssues,
  parseBytes,
  parseLabels,
  parseSeconds,
  toBigqueryParams,
  type BigqueryForm,
} from "./params";

// The lane's params (`BigqueryParams`) as the form keeps them: the cost guard with a unit, the
// timeout in seconds, projects as one line and labels as lines, and `BigqueryParams::validate`'s
// checks in its words.

const form = (over: Partial<BigqueryForm> = {}): BigqueryForm => ({
  ...bigqueryParamsOf(null),
  keyFilePath: "/keys/sa.json",
  ...over,
});

describe("the BigQuery params", () => {
  it("start from the backend's defaults: a key file, 1 GB billed at most, the cache on", () => {
    expect(bigqueryParamsOf(null)).toEqual({
      auth: "serviceAccount",
      keyFilePath: "",
      location: "",
      defaultDataset: "",
      extraProjects: "",
      maximumBytesBilled: "1 GB",
      useQueryCache: true,
      jobTimeoutSecs: "",
      labels: "",
      apiEndpoint: "",
    });
    expect(DEFAULT_MAXIMUM_BYTES_BILLED).toBe(1_073_741_824);
  });

  it("read bytes with units in 1024 steps and write the largest unit that divides them", () => {
    expect(parseBytes("1 GB")).toBe(2 ** 30);
    expect(parseBytes("500mb")).toBe(500 * 2 ** 20);
    expect(parseBytes("2 GiB")).toBe(2 ** 31);
    expect(parseBytes("1.5 KB")).toBe(1536);
    expect(parseBytes("12345")).toBe(12345);
    expect(parseBytes("12 PB")).toBeNull();
    expect(parseBytes("lots")).toBeNull();
    expect(parseBytes("9999999 TB")).toBeNull();
    expect(formatBytes(2 ** 30)).toBe("1 GB");
    expect(formatBytes(3 * 2 ** 40)).toBe("3 TB");
    expect(formatBytes(1536)).toBe("1536");
    expect(parseSeconds("90")).toBe(90_000);
    expect(parseSeconds("1.25")).toBe(1250);
    expect(parseSeconds("1.2345")).toBeNull();
  });

  it("read labels as key = value lines and say which line is none", () => {
    expect(parseLabels("team = data\n\nenv: dev")).toEqual({
      labels: { team: "data", env: "dev" },
      bad: [],
    });
    expect(parseLabels("team = data\noops")).toEqual({
      labels: { team: "data" },
      bad: [{ line: 2, text: "oops" }],
    });
    expect(labelProblem("team", "data-2")).toBeNull();
    expect(labelProblem("team", "")).toBeNull();
    expect(labelProblem("Team", "x")).toMatch(/^The label “Team” must use lower-case/);
    expect(labelProblem("1x", "x")).not.toBeNull();
    expect(labelProblem("k", "a".repeat(64))).not.toBeNull();
  });

  it("send the bearer only over HTTPS, or plain HTTP to this machine (`credentials_may_go_to`)", () => {
    expect(endpointOk("https://bigquery.googleapis.com/bigquery/v2")).toBe(true);
    expect(endpointOk("http://localhost:9050/bigquery/v2")).toBe(true);
    expect(endpointOk("http://127.0.0.1:9050")).toBe(true);
    expect(endpointOk("http://127.1:9050")).toBe(true);
    expect(endpointOk("http://[::1]:9050")).toBe(true);
    expect(endpointOk("http://localhost.example.com")).toBe(false);
    expect(endpointOk("http://10.0.0.5:9050")).toBe(false);
    expect(endpointOk("ftp://localhost")).toBe(false);
    expect(endpointOk("not a url")).toBe(false);
    // The client's path would land in the query or fragment.
    for (const url of [
      "https://bq.example/v2?x=/bigquery/v2",
      "https://bq.example/v2?",
      "https://bq.example/v2#frag",
      "https://bigquery.googleapis.com/bigquery/v2?alt=media",
      "http://localhost:9050/#/bigquery/v2",
    ]) {
      expect(endpointIsBase(url), url).toBe(false);
      expect(endpointOk(url), url).toBe(false);
      expect(googleEndpointOk(url), url).toBe(false);
    }
  });

  it("check what `validate` checks, in its words", () => {
    expect(paramIssues(form())).toEqual([]);
    expect(paramIssues(form({ keyFilePath: "" }))).toEqual([
      { key: "keyFilePath", message: "Choose the service account's key file (JSON)" },
    ]);
    expect(paramIssues(form({ auth: "applicationDefault", keyFilePath: "" }))).toEqual([]);
    expect(paramIssues(form({ maximumBytesBilled: "0" }))[0]?.message).toBe(
      "Maximum bytes billed must be more than 0 (or empty for no limit)",
    );
    expect(paramIssues(form({ maximumBytesBilled: "huge" }))[0]?.key).toBe("maximumBytesBilled");
    expect(paramIssues(form({ maximumBytesBilled: "" }))).toEqual([]);
    expect(paramIssues(form({ jobTimeoutSecs: "soon" }))).toEqual([
      { key: "jobTimeoutSecs", message: "Seconds (empty for none)" },
    ]);
    expect(paramIssues(form({ defaultDataset: "my-data" }))).toEqual([
      {
        key: "defaultDataset",
        message: "“my-data” is no dataset name (letters, digits and underscores)",
      },
    ]);
    expect(paramIssues(form({ labels: "team = Data" }))[0]?.key).toBe("labels");
    expect(paramIssues(form({ labels: "team data" }))[0]?.message).toBe(
      "Line 1: write each label as key = value",
    );
    expect(
      paramIssues(form({ auth: "accessToken", apiEndpoint: "https://bq.example/v2?a=b" })),
    ).toEqual([
      {
        key: "apiEndpoint",
        message:
          "The API endpoint “https://bq.example/v2?a=b” must be a base URL, without a query (?) or fragment (#)",
      },
    ]);
    expect(paramIssues(form({ apiEndpoint: "http://emulator:9050" }))[0]?.message).toBe(
      "The API endpoint “http://emulator:9050” must be an https URL (plain http only to this machine)",
    );
    expect(paramIssues(form({ apiEndpoint: "https://evil.example/bigquery/v2" }))).toEqual([
      {
        key: "apiEndpoint",
        message:
          "With a key file or Application Default Credentials, the API endpoint “https://evil.example/bigquery/v2” must be a googleapis.com host (or this machine); another endpoint needs an access token",
      },
    ]);
    expect(
      paramIssues(form({ auth: "applicationDefault", apiEndpoint: "https://evil.example" }))[0]
        ?.key,
    ).toBe("apiEndpoint");
    expect(paramIssues(form({ auth: "accessToken", apiEndpoint: "https://evil.example" }))).toEqual(
      [],
    );
    expect(paramIssues(form({ keyFilePath: "\\\\attacker.example\\s\\k.json" }))).toEqual([
      {
        key: "keyFilePath",
        message:
          "The key file “\\\\attacker.example\\s\\k.json” must be a full path on this machine (C:\\keys\\sa.json or /keys/sa.json), not a network share",
      },
    ]);
  });

  it("send a Google sign-in only to Google's hosts or this machine", () => {
    for (const ok of [
      "https://bigquery.googleapis.com/bigquery/v2",
      "https://bigquery-psc.p.googleapis.com/bigquery/v2",
      "HTTPS://BigQuery.GoogleApis.com/bigquery/v2",
      "http://localhost:9050/bigquery/v2",
      "https://127.0.0.1:9050",
      "http://[::1]:9050",
    ])
      expect(googleEndpointOk(ok), ok).toBe(true);
    for (const bad of [
      "https://evil.example/bigquery/v2",
      "https://googleapis.com.evil.example",
      "https://evilgoogleapis.com/bigquery/v2",
      "https://bigquery.googleapis.com@evil.example/bigquery/v2",
      "https://evil.example/bigquery.googleapis.com",
      "https://evil.example\\@bigquery.googleapis.com",
      "https://bigquery.googleapis.com./bigquery/v2",
      "http://bigquery.googleapis.com/bigquery/v2",
      "http://localhost.evil.example",
      "ftp://bigquery.googleapis.com",
      "",
    ])
      expect(googleEndpointOk(bad), bad).toBe(false);
  });

  it("read key files only from this machine, never from a share", () => {
    for (const ok of ["C:\\keys\\sa.json", "d:/sa.json", "/keys/sa.json"])
      expect(localFilePath(ok), ok).toBe(true);
    for (const bad of [
      "\\\\attacker.example\\s\\k.json",
      "//attacker.example/s/k.json",
      "\\/attacker.example/s/k.json",
      "\\\\?\\UNC\\attacker.example\\s\\k.json",
      "\\\\.\\pipe\\x",
      "\\??\\UNC\\attacker.example\\s\\k.json",
      "keys\\k.json",
      "C:k.json",
      "~/keys/sa.json",
    ])
      expect(localFilePath(bad), bad).toBe(false);
  });

  it("save the sign-in and the cost guard always, the rest only when set", () => {
    expect(toBigqueryParams(form())).toEqual({
      auth: "serviceAccount",
      keyFilePath: "/keys/sa.json",
      maximumBytesBilled: DEFAULT_MAXIMUM_BYTES_BILLED,
    });
    const full = toBigqueryParams(
      form({
        location: "asia-southeast2",
        defaultDataset: "shop",
        extraProjects: "bigquery-public-data, , other-proj",
        maximumBytesBilled: "",
        useQueryCache: false,
        jobTimeoutSecs: "90",
        labels: "team = data\nenv = dev",
        apiEndpoint: "http://localhost:9050/bigquery/v2",
      }),
    );
    expect(full).toEqual({
      auth: "serviceAccount",
      keyFilePath: "/keys/sa.json",
      location: "asia-southeast2",
      defaultDataset: "shop",
      extraProjects: ["bigquery-public-data", "other-proj"],
      // `null`: no limit, which differs from a missing guard (the backend's 1 GiB).
      maximumBytesBilled: null,
      useQueryCache: false,
      jobTimeoutMs: 90_000,
      labels: { team: "data", env: "dev" },
      apiEndpoint: "http://localhost:9050/bigquery/v2",
    });
    // A token sign-in keeps no key file.
    expect(toBigqueryParams(form({ auth: "accessToken" }))).not.toHaveProperty("keyFilePath");
    // Read back from what a profile stores, the same form again.
    const back = bigqueryParamsOf(full as Record<string, unknown>);
    expect(back).toMatchObject({
      extraProjects: "bigquery-public-data, other-proj",
      maximumBytesBilled: "",
      jobTimeoutSecs: "90",
      labels: "team = data\nenv = dev",
      useQueryCache: false,
    });
    expect(toBigqueryParams(back)).toEqual(full);
  });

  it("read stored values of the wrong type as unset", () => {
    expect(
      bigqueryParamsOf({
        auth: "kerberos",
        keyFilePath: 7,
        extraProjects: ["a", 3],
        maximumBytesBilled: 2 ** 31,
        labels: { team: "data", n: 1 },
      }),
    ).toMatchObject({
      auth: "serviceAccount",
      keyFilePath: "",
      extraProjects: "a",
      maximumBytesBilled: "2 GB",
      labels: "team = data",
    });
  });
});
