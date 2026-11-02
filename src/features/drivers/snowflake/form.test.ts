import { describe, expect, it } from "vitest";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  profileToForm,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import { DESCRIPTORS } from "@/features/drivers/catalog";
import lane from ".";
import { patTooLong } from "./form";
import {
  baseUrlProblem,
  localFilePath,
  normalizeAccount,
  parseSessionParameters,
  snowflakeParamsOf,
  toSnowflakeParams,
  valueOf,
} from "./params";

// The Snowflake form: what it saves (the account as `normalize_account` writes it, the sign-in,
// and only the settings that are set), what it reads back, and its checks next to the schema's,
// in the words of `SnowflakeParams::from_driver_params`.

const ui = lane.kinds!.snowflake!;

const params = (p: Record<string, unknown>) => ({ ...ui.defaults!().driverParams, ...p });

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "snowflake",
  user: "TABLER_USER",
  database: "TABLER_DEMO",
  driverParams: params({ account: "myorg-tabler", privateKeyPath: "/keys/rsa_key.p8" }),
  ...values,
});

/** The schema's issues and the lane's, as `path: message`. */
function issues(values: ConnectionFormValues): string[] {
  const r = connectionFormSchema.safeParse(values);
  const shared = r.success ? [] : r.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`);
  return [...shared, ...(ui.validate?.(values) ?? []).map((i) => `${i.path}: ${i.message}`)];
}

const input = (values: ConnectionFormValues) => formToProfileInput(values, null, "id-1", ui);
const saved = (values: ConnectionFormValues) => input(values).profile;

describe("the Snowflake form", () => {
  it("starts on a key pair, with every row its own and no SSL modes", () => {
    expect(ui.rows).toEqual([]);
    expect(ui.sslModes).toBeNull();
    expect(ui.defaults!()).toEqual({
      host: "",
      port: "",
      ssl: "verify-full",
      passwordMode: "keychain",
      driverParams: {
        account: "",
        auth: "keyPair",
        privateKeyPath: "",
        mfa: false,
        warehouse: "",
        role: "",
        schema: "",
        baseUrl: "",
        sessionParameters: "",
        statementTimeoutSecs: "",
      },
    });
    expect(ui.url!.schemes).toEqual(DESCRIPTORS.snowflake.urlSchemes);
    expect(DESCRIPTORS.snowflake.extraSecrets).toEqual(["keyPassphrase"]);
  });

  it("saves the account as the backend reads it, the key file, and only what is set", () => {
    const p = saved(
      form({
        driverParams: params({
          account: "https://MyOrg-Tabler.snowflakecomputing.com/console#/",
          privateKeyPath: "/keys/rsa_key.p8",
          mfa: true,
          warehouse: "COMPUTE_WH",
          sessionParameters:
            "query_tag = tabler\nWEEK_START = 1\nQUOTED = '1'\nUSE_CACHED_RESULT = false",
          statementTimeoutSecs: "300",
        }),
      }),
    );
    expect(p.host).toBeNull();
    expect(p.port).toBeNull();
    expect(p.ssh).toBeNull();
    expect(p.options.passwordMode).toBe("keychain");
    // MFA belongs to the password sign-in.
    expect(p.options.driverParams).toEqual({
      account: "myorg-tabler",
      auth: "keyPair",
      privateKeyPath: "/keys/rsa_key.p8",
      warehouse: "COMPUTE_WH",
      sessionParameters: {
        QUERY_TAG: "tabler",
        WEEK_START: 1,
        QUOTED: "1",
        USE_CACHED_RESULT: false,
      },
      statementTimeoutSecs: 300,
    });
    const back = profileToForm(p, ui);
    expect(back.driverParams).toEqual({
      account: "myorg-tabler",
      auth: "keyPair",
      privateKeyPath: "/keys/rsa_key.p8",
      mfa: false,
      warehouse: "COMPUTE_WH",
      role: "",
      schema: "",
      baseUrl: "",
      sessionParameters:
        "QUERY_TAG = tabler\nWEEK_START = 1\nQUOTED = '1'\nUSE_CACHED_RESULT = FALSE",
      statementTimeoutSecs: "300",
    });
    expect(saved({ ...form(), ...back }).options.driverParams).toEqual(p.options.driverParams);
  });

  it("keeps the key's passphrase for a key pair and the secret for the other sign-ins", () => {
    // A key pair keeps the Keychain mode for its passphrase, and never a password.
    const key = input(form({ keyPassphrase: "pp", password: "" }));
    expect(key.profile.options.passwordMode).toBe("keychain");
    expect(key.keyPassphrase).toBe("pp");
    expect(key.password).toBeNull();
    const asked = saved(form({ passwordMode: "command", passwordCommand: "x" }));
    expect(asked.options.passwordMode).toBe("keychain");
    expect(asked.options.passwordCommand).toBeNull();
    const pat = input(
      form({ password: "tok", driverParams: params({ account: "a", auth: "pat" }) }),
    );
    expect(pat.password).toBe("tok");
    expect(pat.profile.options.driverParams).toEqual({ account: "a", auth: "pat" });
    const oauth = saved(
      form({
        passwordMode: "command",
        passwordCommand: "my-idp token",
        driverParams: params({ account: "a", auth: "oauth" }),
      }),
    );
    expect(oauth.options.passwordMode).toBe("command");
    expect(oauth.options.passwordCommand).toBe("my-idp token");
    const mfa = saved(
      form({ driverParams: params({ account: "a", auth: "password", mfa: true }) }),
    );
    expect(mfa.options.driverParams).toEqual({ account: "a", auth: "password", mfa: true });
  });

  it("reads the params leniently: the wrong type counts as unset", () => {
    expect(
      snowflakeParamsOf({ auth: "sso", account: 3, mfa: "yes", sessionParameters: [1] }),
    ).toEqual(snowflakeParamsOf(null));
    expect(snowflakeParamsOf({ statementTimeoutSecs: 60 }).statementTimeoutSecs).toBe("60");
    expect(toSnowflakeParams(snowflakeParamsOf(null))).toEqual({ account: "", auth: "keyPair" });
  });

  it("checks the values as the backend does, in its words", () => {
    expect(issues(form())).toEqual([]);
    expect(issues(form({ user: "", driverParams: params({ privateKeyPath: "/k.p8" }) }))).toEqual([
      "user: User is required",
      "driverParams.account: account is required",
    ]);
    expect(
      issues(form({ driverParams: params({ account: "x;DROP", privateKeyPath: "" }) })),
    ).toEqual([
      "driverParams.account: “x;DROP” is no Snowflake account identifier (orgname-account_name or locator.region)",
      "driverParams.privateKeyPath: Key pair sign-in needs the private key file",
    ]);
    expect(
      issues(
        form({ driverParams: params({ account: "a", privateKeyPath: "\\\\host\\share\\k.p8" }) }),
      ),
    ).toEqual([
      "driverParams.privateKeyPath: The key file “\\\\host\\share\\k.p8” must be a full path on this machine (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share",
    ]);
    // Two leading separators in any mix are a share (Windows reads `/` as `\`), and `?` a
    // device prefix; a single `/` or a drive letter is local.
    for (const share of [
      "/\\host\\share\\k.p8",
      "\\/host/share/k.p8",
      "/\\?\\UNC\\host\\s\\k",
      "\\??\\UNC\\host\\s\\k",
      "/??/UNC/host/s/k",
      "\\keys\\k.p8",
      "keys/k.p8",
    ])
      expect(
        issues(form({ driverParams: params({ account: "a", privateKeyPath: share }) })),
        share,
      ).toEqual([
        `driverParams.privateKeyPath: The key file “${share}” must be a full path on this machine (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share`,
      ]);
    for (const local of ["/keys/k.p8", "C:\\keys\\k.p8", "d:/keys/k.p8"])
      expect(localFilePath(local), local).toBe(true);
    // A key pair signs in only at Snowflake's endpoint or this machine's.
    expect(
      issues(
        form({
          driverParams: params({
            account: "a",
            privateKeyPath: "/keys/k.p8",
            baseUrl: "https://attacker.example",
          }),
        }),
      ),
    ).toEqual([
      "driverParams.baseUrl: Snowflake endpoint “https://attacker.example”: a key pair signs in only at Snowflake's own endpoint (*.snowflakecomputing.com) or a test server on this machine (another host could sign in as you with the token)",
    ]);
    expect(
      issues(
        form({
          driverParams: params({
            account: "a",
            auth: "pat",
            baseUrl: "https://proxy.example",
          }),
        }),
      ),
    ).toEqual([]);
    expect(
      issues(
        form({
          driverParams: params({
            account: "a",
            privateKeyPath: "C:\\keys\\k.p8",
            baseUrl: "http://example.com",
            sessionParameters: "QUERY_TAG = x\nX; DROP = 1",
            statementTimeoutSecs: "1h",
          }),
        }),
      ),
    ).toEqual([
      "driverParams.baseUrl: Snowflake endpoint “http://example.com”: use https:// (plain http only reaches a test server on this machine)",
      "driverParams.sessionParameters: Line 2 is not NAME = value (a name of letters, digits and _)",
      "driverParams.statementTimeoutSecs: Seconds, up to 604800 (7 days); empty for the account's",
    ]);
    // A PAT, an OAuth token or a password is the sign-in: there must be one.
    expect(
      issues(form({ passwordMode: "none", driverParams: params({ account: "a", auth: "pat" }) })),
    ).toEqual([
      "passwordMode: The access token is the sign-in: store it, ask for it or run a command",
    ]);
  });

  it("reads every pasted form of the account identifier (normalize_account)", () => {
    for (const [pasted, want] of [
      ["myorg-myaccount", "myorg-myaccount"],
      ["MYORG-MY_ACCOUNT", "myorg-my_account"],
      ["https://myorg-myaccount.snowflakecomputing.com/", "myorg-myaccount"],
      [
        "HTTPS://XY12345.us-east-2.aws.snowflakecomputing.com:443/console#/",
        "xy12345.us-east-2.aws",
      ],
      ["xy12345.us-east-1.privatelink", "xy12345.us-east-1.privatelink"],
      ["  xy12345  ", "xy12345"],
      ["xy12345.cn-north-1.snowflakecomputing.cn", "xy12345.cn-north-1"],
    ])
      expect(normalizeAccount(pasted!), pasted).toBe(want);
    for (const bad of [
      "",
      "https://",
      "a..b",
      "a b",
      "x;DROP",
      "-x",
      ".snowflakecomputing.com",
      "ééééé",
      "日本語アカウント",
    ])
      expect(normalizeAccount(bad), bad).toBeNull();
  });

  it("takes an endpoint as scheme and host, plain http only to this machine (base_url)", () => {
    for (const ok of [
      "https://xy12345.us-east-1.privatelink.snowflakecomputing.com",
      "http://127.0.0.1:8080/",
      "http://127.1:8080",
      "http://localhost:8080",
      "HTTP://LocalHost",
      "http://[::1]:8080",
      "https://example.com",
    ])
      expect(baseUrlProblem(ok), ok).toBeNull();
    for (const bad of [
      "ftp://x",
      "https://u:p@x",
      "https://x/path",
      "https://x/?a=1",
      "https://x/?",
      "not a url",
      "http://example.com",
      "http://10.0.0.5:8080",
      "http://localhost.example.com",
      "http://127.0.0.1.nip.io",
      "http://[::ffff:10.0.0.5]",
      "http://0.0.0.0:8080",
    ])
      expect(baseUrlProblem(bad), bad).not.toBeNull();
  });

  it("reads session parameters as typed: quotes keep text", () => {
    expect(valueOf("'TRUE'")).toBe("TRUE");
    expect(valueOf("it''s")).toBe("it''s");
    expect(valueOf("'it''s'")).toBe("it's");
    expect(valueOf(" 12.5 ")).toBe(12.5);
    expect(valueOf("True")).toBe(true);
    expect(parseSessionParameters("\n a = 1 \n\nb=c=d\nbad line").params).toEqual({
      A: 1,
      B: "c=d",
    });
  });

  it("warns that Windows Credential Manager cannot keep a PAT past 1,280 characters", () => {
    const long = form({
      password: "p".repeat(1281),
      driverParams: params({ account: "a", auth: "pat" }),
    });
    expect(patTooLong(long, true)).toBe(true);
    expect(patTooLong(long, false)).toBe(false);
    expect(patTooLong({ ...long, password: "p".repeat(1280) }, true)).toBe(false);
    // Command Line and Ask every time keep nothing.
    expect(patTooLong({ ...long, passwordMode: "command" }, true)).toBe(false);
    expect(
      patTooLong({ ...long, driverParams: params({ account: "a", auth: "oauth" }) }, true),
    ).toBe(false);
  });

  it("sums a profile up as user@account · warehouse · database.schema · sign-in", () => {
    expect(
      ui.summary!(
        saved(
          form({
            driverParams: params({
              account: "myorg-tabler",
              privateKeyPath: "/k.p8",
              warehouse: "COMPUTE_WH",
              schema: "PUBLIC",
            }),
          }),
        ),
      ),
    ).toBe("TABLER_USER@myorg-tabler · COMPUTE_WH · TABLER_DEMO.PUBLIC · key pair");
    expect(
      ui.summary!(
        saved(
          form({
            database: "",
            driverParams: params({ account: "a", auth: "password", mfa: true }),
          }),
        ),
      ),
    ).toBe("TABLER_USER@a · password + MFA");
  });
});
