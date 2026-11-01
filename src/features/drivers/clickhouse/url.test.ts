import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import lane from ".";
import { formatClickhouseUrl, parseClickhouseUrl } from "./url";

// `clickhouse://` and `clickhouses://` URLs read into the form (a password into the secret), and
// a profile written as a URL that reads back to the same profile.

const ui = lane.kinds!.clickhouse!;
const url = ui.url!;

/** What a URL makes of a new form, then saved: the profile and the secret. */
function saved(text: string): { profile: ConnectionProfile; password: string | null } {
  const parsed = url.parse(text);
  if (!parsed) throw new Error(`not read: ${text}`);
  const values: ConnectionFormValues = {
    ...DEFAULT_FORM,
    ...ui.defaults!(),
    driver: "clickhouse",
    name: "n",
    ...parsed.values,
    driverParams: { ...ui.defaults!().driverParams, ...parsed.values.driverParams },
  };
  return {
    profile: formToProfileInput(values, null, "id-1", ui).profile,
    password: parsed.password,
  };
}

describe("clickhouse:// URLs", () => {
  it("use the descriptor's scheme, and clickhouses:// for HTTPS", () => {
    expect(url.schemes).toEqual(["clickhouse", "clickhouses"]);
  });

  it("read the user, password, host, port and database", () => {
    const p = parseClickhouseUrl("clickhouse://app:p%40ss@ch.example.com:8124/My%20Db");
    expect(p?.password).toBe("p@ss");
    expect(p?.values).toMatchObject({
      host: "ch.example.com",
      port: "8124",
      user: "app",
      database: "My Db",
      ssl: "disable",
      driverParams: { certificateAuth: false, roles: "", settings: "", sessionTimeoutSecs: "" },
    });
    expect(parseClickhouseUrl("CLICKHOUSE://[::1]")?.values).toMatchObject({
      host: "::1",
      port: "",
      user: "",
      database: "",
    });
    expect(parseClickhouseUrl("clickhouse://u@h")?.password).toBeNull();
  });

  it("read HTTPS from the scheme, sslmode, secure and skip_verify", () => {
    const ssl = (text: string) => parseClickhouseUrl(text)?.values.ssl;
    expect(ssl("clickhouses://h")).toBe("verify-full");
    expect(ssl("clickhouses://h?skip_verify=true")).toBe("require");
    expect(ssl("clickhouse://h?secure=true")).toBe("verify-full");
    expect(ssl("clickhouse://h?ssl=1&skipVerify=1")).toBe("require");
    expect(ssl("clickhouses://h?secure=false")).toBe("disable");
    expect(ssl("clickhouse://h?sslmode=verify-ca")).toBe("verify-ca");
    // `entry::certificate_check`: Prefer checks fully.
    expect(ssl("clickhouse://h?sslmode=prefer")).toBe("verify-full");
    expect(ssl("clickhouses://h?sslmode=disable")).toBe("disable");
    // A TLS value that cannot be read is no URL, rather than one in plain text.
    expect(parseClickhouseUrl("clickhouse://h?sslmode=sometimes")).toBeNull();
    expect(parseClickhouseUrl("clickhouse://h?secure=maybe")).toBeNull();
    expect(parseClickhouseUrl("clickhouses://h?skip_verify=perhaps")).toBeNull();
    expect(parseClickhouseUrl("clickhouse://h?sslmode=constructor")).toBeNull();
  });

  it("read roles, the session timeout, a certificate sign-in and settings", () => {
    const p = parseClickhouseUrl(
      "clickhouse://h/db?role=analyst&roles=a,b&session_timeout=600&certificateAuth=true&max_execution_time=60&join_use_nulls=1&connect_timeout=7",
    );
    expect(p?.values).toMatchObject({
      connectTimeoutSecs: "7",
      driverParams: {
        certificateAuth: true,
        roles: "analyst, a, b",
        settings: "max_execution_time = 60\njoin_use_nulls = 1",
        sessionTimeoutSecs: "600",
      },
    });
    // An out-of-range timeout is left to the server's default.
    expect(
      parseClickhouseUrl("clickhouse://h?sessionTimeout=0")?.values.driverParams,
    ).toMatchObject({
      sessionTimeoutSecs: "",
    });
    // The app's own keys (name, table, …) are no settings.
    expect(
      parseClickhouseUrl("clickhouse://h?name=Prod&statusColor=ff0000&table=t")?.values
        .driverParams,
    ).toMatchObject({ settings: "" });
  });

  it("are refused with a setting the backend refuses", () => {
    for (const bad of ["readonly=0", "dialect=prql", "param_x=1", "max-threads=2", "profile=web"])
      expect(parseClickhouseUrl(`clickhouse://h?${bad}`), bad).toBeNull();
  });

  it("are refused when malformed", () => {
    for (const bad of [
      "clickhouse://",
      "clickhouse://u@",
      "clickhouse://h:x",
      "clickhouse://%zz@h",
      "clickhouse://u:%zz@h",
      "clickhouse://h/a/b",
      "clickhouse://h/%zz",
      "mysql://h",
      "clickhousex://h",
    ])
      expect(parseClickhouseUrl(bad), bad).toBeNull();
  });

  it("write a profile that reads back to the same profile, for every SSL mode", () => {
    for (const ssl of ["disable", "require", "verify-ca", "verify-full"] as const) {
      const first = saved(
        `clickhouse://app%20user@[::1]:8443/an%20db?sslmode=${ssl}&role=analyst&role=ops&sessionTimeout=900&certificateAuth=true&max_threads=4&connect_timeout=5`,
      );
      const text = url.format(first.profile);
      expect(text, ssl).not.toBeNull();
      const again = saved(text!);
      expect(again.profile, text!).toEqual(first.profile);
      expect(again.password).toBeNull();
    }
    const https = saved("clickhouses://alice:secret@ch.example.com/shop");
    expect(https.password).toBe("secret");
    expect(url.format(https.profile)).toBe(
      "clickhouse://alice@ch.example.com/shop?sslmode=verify-full",
    );
  });

  it("write the plain profile without a query, and never a password", () => {
    const { profile } = saved("clickhouse://alice:secret@ch.example.com:8123/shop");
    expect(formatClickhouseUrl(profile)).toBe("clickhouse://alice@ch.example.com:8123/shop");
    expect(formatClickhouseUrl({ ...profile, host: null })).toBeNull();
    expect(formatClickhouseUrl({ ...profile, host: "a b" })).toBeNull();
  });

  it("write no URL for a setting named like a key the URL reads otherwise", () => {
    const { profile } = saved("clickhouse://h");
    const withSetting = (name: string): ConnectionProfile => ({
      ...profile,
      options: { ...profile.options, driverParams: { settings: [{ name, value: "1" }] } },
    });
    expect(formatClickhouseUrl(withSetting("max_threads"))).toBe("clickhouse://h?max_threads=1");
    expect(formatClickhouseUrl(withSetting("name"))).toBeNull();
    expect(formatClickhouseUrl(withSetting("secure"))).toBeNull();
  });
});
