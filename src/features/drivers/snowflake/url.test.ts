import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { parseConnectionUrl } from "@/features/connections/url";
import { CORE_KINDS } from "@/features/drivers/catalog";
import { awaitKindUi, registerLaneOverride } from "@/features/drivers/registry";
import { useAppInfoStore } from "@/stores/appInfo";
import type { ConnectionProfile } from "@/lib/ipc";
import lane from ".";
import { formatSnowflakeUrl, parseSnowflakeUrl } from "./url";

// Snowflake URLs: the real-server suite's `snowflake://USER[:SECRET]@ACCOUNT/DATABASE/SCHEMA?…`
// and JDBC's keys read, a secret kept out of the fields, a clicked link never picking the user's
// key, and every profile written as a URL that reads back to the same profile.

const ui = lane.kinds!.snowflake!;

const base = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "snowflake",
  ...values,
});

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

/** The profile a pasted URL makes, through the form, as "New connection from URL…" saves it. */
function profileOf(url: string): ConnectionProfile {
  const parsed = parseSnowflakeUrl(url, "paste");
  if (!parsed) throw new Error(`not read: ${url}`);
  return saved(base({ ...parsed.values }));
}

const formParams = (over: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...over });

describe("Snowflake URLs", () => {
  it("read the real-server suite's form, the secret to the secret", () => {
    expect(
      parseSnowflakeUrl(
        "snowflake://TABLER_USER:p%40t@MyOrg-Tabler/TABLER_DEMO/PUBLIC?warehouse=COMPUTE_WH&role=SYSADMIN&auth=pat",
        "paste",
      ),
    ).toEqual({
      values: {
        host: "",
        port: "",
        user: "TABLER_USER",
        database: "TABLER_DEMO",
        name: "myorg-tabler",
        driverParams: formParams({
          account: "myorg-tabler",
          auth: "pat",
          warehouse: "COMPUTE_WH",
          role: "SYSADMIN",
          schema: "PUBLIC",
        }),
      },
      password: "p@t",
    });
    // A key file makes a key pair, its passphrase the second secret; no password.
    const key = parseSnowflakeUrl(
      "snowflake://u@acct/DB/S?privateKeyPath=%2Fkeys%2Frsa_key.p8&passphrase=pp",
      "paste",
    );
    expect(key?.values.driverParams).toMatchObject({
      auth: "keyPair",
      privateKeyPath: "/keys/rsa_key.p8",
    });
    expect(key?.values.keyPassphrase).toBe("pp");
    expect(key?.password).toBeNull();
    // A secret without a sign-in reads as a PAT; nothing at all as Snowflake's default key pair.
    expect(parseSnowflakeUrl("snowflake://u:t@acct", "paste")?.values.driverParams).toMatchObject({
      auth: "pat",
    });
    expect(parseSnowflakeUrl("snowflake://u@acct", "paste")?.values.driverParams).toMatchObject({
      auth: "keyPair",
    });
  });

  it("read JDBC's keys and authenticators, the account as a host", () => {
    const jdbc = parseSnowflakeUrl(
      "jdbc:snowflake://xy12345.us-east-2.aws.snowflakecomputing.com:443/?user=ana&db=SALES&schema=RAW&warehouse=WH&role=R&authenticator=username_password_mfa&loginTimeout=60&QUERY_TAG=etl&WEEK_START=1",
      "paste",
    );
    expect(jdbc?.values).toMatchObject({
      user: "ana",
      database: "SALES",
      connectTimeoutSecs: "60",
      driverParams: {
        account: "xy12345.us-east-2.aws",
        auth: "password",
        mfa: true,
        schema: "RAW",
        warehouse: "WH",
        role: "R",
        sessionParameters: "QUERY_TAG = etl\nWEEK_START = 1",
      },
    });
    for (const [authenticator, auth] of [
      ["snowflake_jwt", "keyPair"],
      ["SNOWFLAKE_JWT", "keyPair"],
      ["oauth", "oauth"],
      ["programmatic_access_token", "pat"],
      ["snowflake", "password"],
    ])
      expect(
        parseSnowflakeUrl(`snowflake://u:x@a?authenticator=${authenticator}`, "paste")?.values
          .driverParams,
        authenticator,
      ).toMatchObject({ auth });
    // `token` is the secret of an OAuth URL.
    expect(parseSnowflakeUrl("snowflake://u@a?auth=oauth&token=tk", "paste")?.password).toBe("tk");
  });

  it("refuse what is no Snowflake URL, or one whose values the backend would refuse", () => {
    for (const bad of [
      "postgres://u@h/db",
      "snowflake://",
      "snowflake://u@x;DROP/db",
      "snowflake://u@a?auth=externalbrowser",
      "snowflake://u@a?mfa=maybe",
      "snowflake://u@a?statementTimeout=1h",
      "snowflake://u@a?baseUrl=http://example.com",
      "snowflake://u@a?baseUrl=https://h/path",
      // A share would sign in to its host before any check runs.
      "snowflake://u@a?privateKeyPath=%5C%5Chost%5Cshare%5Ck.p8",
      "snowflake://u@a?privateKeyPath=//host/share/k.p8",
      // Windows reads `/` as `\`: `/\host\share` is `\\host\share`.
      "snowflake://u@acct/DB?auth=keyPair&privateKeyPath=%2F%5Chost%5Cshare%5Ck.p8",
      "snowflake://u@a?privateKeyPath=%2F%5C%3F%5CUNC%5Chost%5Cs%5Ck",
      "snowflake://u@a?privateKeyPath=%5C%3F%3F%5CUNC%5Chost%5Cs%5Ck",
      // A key pair's JWT never goes to a host other than Snowflake's or this machine's.
      "snowflake://u@a?privateKeyPath=%2Fkeys%2Fk.p8&baseUrl=https://attacker.example",
    ])
      expect(parseSnowflakeUrl(bad, "paste"), bad).toBeNull();
  });

  it("from a clicked link never pick the user's key, nor send a typed secret elsewhere", () => {
    const url =
      "snowflake://victim@acct/DB?auth=keyPair&privateKeyPath=%2FUsers%2Fme%2F.snowflake%2Frsa_key.p8&passphrase=pp";
    for (const source of ["link", undefined] as const) {
      const link = parseSnowflakeUrl(url, source);
      expect(link?.values.driverParams).toMatchObject({ auth: "pat", privateKeyPath: "" });
      expect(link?.values.keyPassphrase).toBeUndefined();
    }
    // A link's session parameters would change how the session reads what the grid writes.
    const formats =
      "snowflake://me@myorg-acct/DB?DATE_INPUT_FORMAT=YYYY-DD-MM&TIMEZONE=Pacific/Apia";
    for (const source of ["link", undefined] as const)
      expect(parseSnowflakeUrl(formats, source)?.values.driverParams).toMatchObject({
        sessionParameters: "",
      });
    expect(parseSnowflakeUrl(formats, "paste")?.values.driverParams).toMatchObject({
      sessionParameters: "DATE_INPUT_FORMAT = YYYY-DD-MM\nTIMEZONE = Pacific/Apia",
    });
    // Another endpoint than Snowflake's (or this machine) only with the link's own secret.
    const away = "snowflake://u@acct?auth=pat&baseUrl=https://collector.example.com";
    expect(parseSnowflakeUrl(away, "link")).toBeNull();
    expect(parseSnowflakeUrl(away, "paste")?.values.driverParams).toMatchObject({
      baseUrl: "https://collector.example.com",
    });
    expect(
      parseSnowflakeUrl(
        "snowflake://u:own@acct?auth=pat&baseUrl=https://collector.example.com",
        "link",
      )?.password,
    ).toBe("own");
    for (const ok of [
      "https://xy12345.us-east-1.privatelink.snowflakecomputing.com",
      "http://127.0.0.1:8080",
    ])
      expect(
        parseSnowflakeUrl(`snowflake://u@acct?auth=pat&baseUrl=${ok}`, "link"),
        ok,
      ).not.toBeNull();
  });

  it("write every profile as a URL that reads back to it, never with a secret", () => {
    const profiles = [
      saved(
        base({
          user: "TABLER_USER",
          database: "TABLER_DEMO",
          connectTimeoutSecs: "45",
          driverParams: formParams({
            account: "myorg-tabler",
            privateKeyPath: "/Users/me/.snowflake/rsa key.p8",
            warehouse: "COMPUTE_WH",
            role: "SYSADMIN",
            schema: "My Schema/2",
            sessionParameters:
              "QUERY_TAG = tabler\nTIMEZONE = Asia/Jakarta\nWEEK_START = 1\nCODE = '007'",
            statementTimeoutSecs: "300",
          }),
          password: "never",
          keyPassphrase: "never",
        }),
      ),
      saved(
        base({
          user: "ana@example.com",
          database: "",
          tag: "production",
          driverParams: formParams({
            account: "xy12345.us-east-2.aws",
            auth: "password",
            mfa: true,
            schema: "RAW",
            baseUrl: "https://xy12345.us-east-2.privatelink.snowflakecomputing.com",
          }),
        }),
      ),
      saved(base({ user: "svc", driverParams: formParams({ account: "a", auth: "oauth" }) })),
    ];
    for (const p of profiles) {
      const url = formatSnowflakeUrl(p)!;
      expect(url, url).toMatch(/^snowflake:\/\//);
      expect(url).not.toContain("never");
      const back = profileOf(url);
      expect(back.options.driverParams, url).toEqual(p.options.driverParams);
      expect([back.user, back.database, back.options.connectTimeoutSecs], url).toEqual([
        p.user,
        p.database,
        p.options.connectTimeoutSecs,
      ]);
    }
    expect(formatSnowflakeUrl(profiles[0]!)).toContain(
      "snowflake://TABLER_USER@myorg-tabler/TABLER_DEMO/My%20Schema%2F2?auth=keyPair",
    );
    // No account, no URL.
    expect(
      formatSnowflakeUrl({
        ...profiles[2]!,
        options: { ...profiles[2]!.options, driverParams: { auth: "pat" } },
      }),
    ).toBeNull();
  });
});

describe("New connection from URL with a snowflake:// URL", () => {
  afterEach(() => {
    registerLaneOverride("snowflake", null);
    useAppInfoStore.setState({ info: null });
  });

  it("reads the lane's part, and the name, colour, tag and Safe Mode every URL carries", async () => {
    useAppInfoStore.setState({
      info: { version: "1", platform: "test", debug: true, kinds: [...CORE_KINDS, "snowflake"] },
    });
    registerLaneOverride("snowflake", lane);
    await awaitKindUi("snowflake");
    const p = saved(
      base({
        name: "Shop",
        color: "red",
        tag: "production",
        safeMode: "alert",
        user: "u",
        driverParams: formParams({ account: "a", auth: "pat" }),
      }),
    );
    const url = formatSnowflakeUrl(p)!;
    // The lane's keys, then the query every URL carries (`profileUrlQuery`).
    expect(url).toMatch(
      /^snowflake:\/\/u@a\?auth=pat&name=Shop&statusColor=[0-9A-F]{6}&env=production&/,
    );
    const parsed = parseConnectionUrl(url);
    expect(parsed.values).toMatchObject({
      driver: "snowflake",
      name: "Shop",
      color: "red",
      tag: "production",
      safeMode: "alert",
      user: "u",
    });
    // A link reads a key pair URL as a token sign-in.
    const link = parseConnectionUrl("snowflake://u@a?privateKeyPath=/keys/k.p8", "link");
    expect(link.values.driverParams).toMatchObject({ auth: "pat", privateKeyPath: "" });
  });
});
