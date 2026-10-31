import { describe, expect, it } from "vitest";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import lane from ".";
import { cloudIdUrl, parseBaseUrl, regionForHost, rootOf } from "./address";
import { ES_SSL_MODES } from "./form";
import { formatElasticUrl, parseElasticUrl } from "./url";

// The Elasticsearch form: what it saves (the params that are set, never another sign-in's), what
// it reads back, its checks next to the schema's (the backend's `validate`), and the
// `elasticsearch://` / `opensearch://` URLs, which read back to the profile they were written from.

const ui = lane.kinds!.elasticsearch!;
const url = ui.url!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "elasticsearch",
  host: "es.example.com",
  password: "pw",
  ...values,
});

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

const save = (values: ConnectionFormValues) => formToProfileInput(values, null, "id-1", ui).profile;

describe("the Elasticsearch form", () => {
  it("starts on 127.0.0.1:9200 as elastic, over checked HTTPS", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "ssl"]);
    const d = ui.defaults!();
    expect(d).toMatchObject({
      host: "127.0.0.1",
      port: "9200",
      ssl: "verify-full",
      user: "elastic",
    });
    expect(d.driverParams).toMatchObject({ auth: "basic", urls: "", showSystemIndices: false });
    expect(ES_SSL_MODES.map(([m]) => m)).toEqual([
      "verify-full",
      "verify-ca",
      "require",
      "prefer",
      "disable",
    ]);
  });

  it("saves the params that are set and reads them back", () => {
    const p = save(
      form({
        driverParams: params({
          urls: " https://a:9200 \n\nhttps://b:9200,",
          pathPrefix: "/es",
          auth: "apiKey",
          apiKeyId: "kid",
          // Another sign-in's settings are not kept.
          awsRegion: "eu-west-1",
          showSystemIndices: true,
          requestTimeoutSecs: "30",
        }),
      }),
    );
    expect(p.driver).toBe("elasticsearch");
    expect(p.database).toBeNull();
    expect(p.options.driverParams).toEqual({
      urls: ["https://a:9200", "https://b:9200"],
      pathPrefix: "/es",
      auth: "apiKey",
      apiKeyId: "kid",
      awsCredentials: "accessKey",
      showSystemIndices: true,
      requestTimeoutSecs: 30,
    });
    const back = profileToForm(p, ui);
    expect(back.driverParams).toMatchObject({
      urls: "https://a:9200\nhttps://b:9200",
      auth: "apiKey",
      apiKeyId: "kid",
      awsRegion: "",
      requestTimeoutSecs: "30",
    });
  });

  it("keeps SigV4's settings with SigV4", () => {
    const p = save(
      form({
        host: "https://search-logs-abc.eu-west-1.es.amazonaws.com",
        driverParams: params({
          auth: "awsSigv4",
          awsCredentials: "profile",
          awsProfile: "prod",
          awsService: "es",
        }),
      }),
    );
    expect(p.options.driverParams).toMatchObject({
      auth: "awsSigv4",
      awsCredentials: "profile",
      awsProfile: "prod",
      awsService: "es",
    });
  });

  it("accepts a plain profile, a Cloud ID and node URLs", () => {
    expect(issues(form())).toEqual([]);
    expect(
      issues(
        form({ host: "", driverParams: params({ cloudId: "c:" + btoa("cloud.es.io$abc$kb") }) }),
      ),
    ).toEqual([]);
    expect(
      issues(form({ host: "", driverParams: params({ urls: "https://a:9200\nhttps://b" }) })),
    ).toEqual([]);
    expect(issues(form({ ssl: "disable", host: "http://localhost:9200/es" }))).toEqual([]);
  });

  it("says what the backend refuses before connecting", () => {
    const one = (values: Partial<ConnectionFormValues>) => issues(form(values));
    expect(one({ host: "" })).toEqual(["host: Enter the host, the URLs or a Cloud ID"]);
    expect(one({ driverParams: params({ urls: "https://a", cloudId: "x:eA==" }) })).toEqual([
      "driverParams.cloudId: Give either a Cloud ID or URLs, not both",
    ]);
    expect(one({ driverParams: params({ cloudId: "nope" }) })).toEqual([
      "driverParams.cloudId: The Cloud ID is not valid",
    ]);
    expect(one({ driverParams: params({ pathPrefix: "/_cluster" }) })[0]).toMatch(
      /^driverParams.pathPrefix: .*starting with _/,
    );
    expect(one({ driverParams: params({ pathPrefix: "es" }) })[0]).toMatch(/must start with \//);
    expect(one({ driverParams: params({ caFingerprint: "abc" }) })[0]).toMatch(/64 digits/);
    expect(one({ driverParams: params({ awsRegion: "Not A Region" }) })[0]).toMatch(
      /not an AWS region/,
    );
    expect(one({ driverParams: params({ requestTimeoutSecs: "0" }) })).toEqual([
      "driverParams.requestTimeoutSecs: 1 to 86400 seconds",
    ]);
    expect(one({ driverParams: params({ urls: "https://u:p@a" }) })[0]).toMatch(
      /^driverParams.urls: .*own fields/,
    );
    expect(one({ host: "https://es/_refresh" })[0]).toMatch(/^host: .*starting with _/);
    expect(one({ driverParams: params({ urls: "ftp://a" }) })[0]).toMatch(/use http:\/\//);
    // Plain HTTP where TLS is asked for.
    expect(one({ host: "http://es:9200" })[0]).toMatch(/^host: http:\/\/es:9200 is plain HTTP/);
    expect(one({ ssl: "prefer", host: "http://es:9200" })).toEqual([]);
    // OpenSearch Serverless, by service or by host.
    expect(
      one({
        driverParams: params({ auth: "awsSigv4", awsService: "aoss", awsRegion: "us-east-1" }),
      })[0],
    ).toMatch(/^driverParams.awsService: OpenSearch Serverless/);
    expect(one({ host: "abc.us-east-1.aoss.amazonaws.com" })[0]).toMatch(
      /^host: OpenSearch Serverless/,
    );
    // SigV4 needs a region the host does not tell.
    expect(one({ driverParams: params({ auth: "awsSigv4" }) })).toEqual([
      "driverParams.awsRegion: AWS SigV4 needs the region (the endpoint's host name does not tell it)",
    ]);
    expect(
      one({
        host: "search-x.eu-west-1.es.amazonaws.com",
        driverParams: params({ auth: "awsSigv4" }),
      }),
    ).toEqual([]);
  });

  it("over SSH names the cluster by the host and port", () => {
    const ssh = { sshEnabled: true, sshHost: "bastion" };
    expect(issues(form(ssh))).toEqual([]);
    expect(
      issues(form({ ...ssh, host: "", driverParams: params({ urls: "https://other:9200" }) }))[0],
    ).toMatch(/^driverParams.urls: Over SSH the tunnel reaches 127.0.0.1:9200 only/);
  });

  it("sums a profile up for the connection list", () => {
    const p = save(form({ port: "9243", driverParams: params({ pathPrefix: "/es" }) }));
    expect(ui.summary!(p)).toBe("es.example.com : 9243 /es");
    const c = save(form({ host: "", driverParams: params({ cloudId: "prod:" + btoa("h$e") }) }));
    expect(ui.summary!(c)).toBe("Cloud prod");
  });
});

describe("addresses", () => {
  it("read node URLs and hosts as the backend does", () => {
    expect(parseBaseUrl("https://es.example.com", false)).toEqual({
      https: true,
      host: "es.example.com",
      port: 443,
      prefix: "",
    });
    expect(parseBaseUrl("https://localhost", false)).toMatchObject({ port: 9200 });
    expect(parseBaseUrl("opensearch://[::1]:9201/os/", false)).toEqual({
      https: false,
      host: "::1",
      port: 9201,
      prefix: "/os",
    });
    expect(parseBaseUrl("es", true)).toMatchObject({ https: true, port: 9200 });
    for (const bad of ["https://h?x", "h#f", "[::1", "h:port", "https://", "a b"])
      expect(parseBaseUrl(bad, true), bad).toHaveProperty("error");
  });

  it("read a Cloud ID and an Amazon host", () => {
    const id = "name:" + btoa("us-central1.gcp.cloud.es.io:9243$abc123$kib");
    expect(rootOf(cloudIdUrl(id)!)).toBe("https://abc123.us-central1.gcp.cloud.es.io:9243");
    expect(cloudIdUrl("name:!!")).toBeNull();
    expect(regionForHost("search-logs-abc123.eu-west-1.es.amazonaws.com")).toBe("eu-west-1");
    expect(regionForHost("es.example.com")).toBeNull();
  });
});

describe("elasticsearch:// and opensearch:// URLs", () => {
  /** What a URL makes of a new form, then saved: the profile and the secret. */
  function saved(text: string): { profile: ConnectionProfile; password: string | null } {
    const parsed = url.parse(text);
    if (!parsed) throw new Error(`not read: ${text}`);
    const values = form({
      ...parsed.values,
      password: "",
      driverParams: { ...ui.defaults!().driverParams, ...parsed.values.driverParams },
    });
    return { profile: save(values), password: parsed.password };
  }

  it("read the user, password, host, port, prefix and TLS for both schemes", () => {
    expect(url.schemes).toEqual(["elasticsearch", "opensearch"]);
    for (const scheme of url.schemes) {
      const p = parseElasticUrl(`${scheme}://elastic:p%40ss@es.example.com:9243/proxy/es?ssl=true`);
      expect(p?.password).toBe("p@ss");
      expect(p?.values).toMatchObject({
        host: "es.example.com",
        port: "9243",
        user: "elastic",
        ssl: "require",
        driverParams: { pathPrefix: "/proxy/es", auth: "basic" },
      });
    }
    expect(parseElasticUrl("elasticsearch://127.0.0.1")?.values).toMatchObject({
      port: "9200",
      ssl: "verify-full",
    });
    expect(parseElasticUrl("elasticsearch://h?ssl=false")?.values.ssl).toBe("disable");
    expect(parseElasticUrl("opensearch://[::1]:9200")?.values.host).toBe("::1");
  });

  it("refuse what they cannot read", () => {
    for (const bad of [
      "elasticsearch://h?auth=kerberos",
      "elasticsearch://h?sslmode=sometimes",
      "elasticsearch://h:99999",
      "elasticsearch://%zz",
      "http://h:9200",
    ])
      expect(parseElasticUrl(bad), bad).toBeNull();
  });

  it("round-trip a profile, its secret aside", () => {
    const profiles = [
      save(form({ user: "elastic", port: "9243" })),
      save(
        form({
          ssl: "require",
          driverParams: params({
            auth: "apiKey",
            apiKeyId: "k1",
            pathPrefix: "/es",
            caFingerprint: "AA".repeat(32),
            showSystemIndices: true,
            requestTimeoutSecs: "15",
          }),
        }),
      ),
      save(
        form({
          host: "",
          port: "",
          user: "AKIA",
          driverParams: params({
            urls: "https://search-x.eu-west-1.es.amazonaws.com\nhttps://b.eu-west-1.es.amazonaws.com",
            auth: "awsSigv4",
            awsRegion: "eu-west-1",
            awsService: "es",
          }),
        }),
      ),
      save(
        form({
          host: "",
          port: "",
          driverParams: params({ cloudId: "prod:" + btoa("h$e"), auth: "bearer" }),
        }),
      ),
      save(form({ host: "::1", ssl: "disable", driverParams: params({ auth: "none" }) })),
    ];
    for (const p of profiles) {
      const text = url.format(p)!;
      const back = saved(text).profile;
      expect(back, text).toMatchObject({
        host: p.host,
        port: p.port,
        ssl: p.ssl,
        options: { driverParams: p.options.driverParams },
      });
      // A user travels only where it signs in.
      expect(back.user ?? "", text).toBe(
        p.options.driverParams?.auth === "basic" || p.options.driverParams?.auth === "awsSigv4"
          ? (p.user ?? "")
          : "",
      );
      // Both schemes read the same profile.
      expect(
        saved(text.replace(/^elasticsearch:/, "opensearch:")).profile.options.driverParams,
      ).toEqual(back.options.driverParams);
    }
    expect(formatElasticUrl(profiles[0]!)).toBe("elasticsearch://elastic@es.example.com:9243");
  });

  it("carry a Host field that holds a URL among the URLs", () => {
    const p = save(form({ ssl: "disable", host: "http://es:9200/x" }));
    const text = url.format(p)!;
    expect(text).toBe("elasticsearch://elastic@?sslmode=disable&urls=http%3A%2F%2Fes%3A9200%2Fx");
    expect(saved(text).profile.options.driverParams).toMatchObject({ urls: ["http://es:9200/x"] });
  });
});
