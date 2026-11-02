import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { SslMode } from "@/lib/ipc";
import lane from ".";
import { formatRedisUrl, parseRedisUrl } from "./url";

// Redis URLs: redis-cli's, redis-py's and TablePlus's forms read, a password kept out of the
// fields, and every profile written as a URL that reads back to the same profile.

const ui = lane.kinds!.redis!;

const base = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "redis",
  host: "cache.example.com",
  ...values,
});

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

/** The profile a URL makes, through the form, as "New connection from URL…" saves it. */
function profileOf(url: string) {
  const parsed = parseRedisUrl(url);
  if (!parsed) throw new Error(`not read: ${url}`);
  return saved(base({ ...parsed.values }));
}

const DEFAULT_PARAMS = {
  scanCount: "",
  previewBytes: "",
  previewElements: "",
  memoryColumn: true,
  keySeparator: "",
  protocol: "auto",
};

describe("Redis URLs", () => {
  it("read user, host, port and DB index, the password to the secret", () => {
    expect(parseRedisUrl("redis://app:s%40cret@cache.example.com:6380/3")).toEqual({
      values: {
        host: "cache.example.com",
        port: "6380",
        user: "app",
        database: "3",
        ssl: "disable",
        driverParams: DEFAULT_PARAMS,
      },
      password: "s@cret",
    });
    // redis-cli's password-only form, the default port, an IPv6 host, `db` in the query.
    expect(parseRedisUrl("redis://:pw@[::1]?db=2")).toMatchObject({
      values: { host: "::1", port: "6379", user: "", database: "2" },
      password: "pw",
    });
    // Userinfo without a ':' is redis-cli's legacy `password@`: the secret, never the user.
    for (const [url, secret] of [
      ["redis://s3cr3t@cache.example.com:6379/0", "s3cr3t"],
      ["rediss://tok%40en@h", "tok@en"],
    ] as const) {
      expect(parseRedisUrl(url), url).toMatchObject({ values: { user: "" }, password: secret });
      const written = formatRedisUrl(profileOf(url))!;
      expect(written, url).not.toContain(secret.replace("@", "%40"));
      expect(written, url).not.toContain(secret);
    }
    expect(parseRedisUrl("redis://app:@h")).toMatchObject({
      values: { user: "app" },
      password: null,
    });
    expect(parseRedisUrl("REDIS://localhost")?.values.database).toBe("");
    expect(parseRedisUrl("redis://h/db7")?.values.database).toBe("db7");
  });

  it("read TLS from rediss:// and how its certificate is checked", () => {
    const ssl = (url: string) => parseRedisUrl(url)?.values.ssl;
    expect(ssl("redis://h")).toBe("disable");
    expect(ssl("rediss://h")).toBe("require");
    expect(ssl("rediss://h?verify=full")).toBe("verify-full");
    expect(ssl("rediss://h?verify=ca")).toBe("verify-ca");
    expect(ssl("rediss://h?ssl_cert_reqs=required")).toBe("verify-full");
    expect(ssl("rediss://h?ssl_cert_reqs=none")).toBe("require");
    // A mode this does not know is no URL: the form's default could be weaker.
    expect(parseRedisUrl("rediss://h?verify=sometimes")).toBeNull();
  });

  it("read the protocol, the lane's params and the connect timeout", () => {
    expect(
      parseRedisUrl(
        "redis://h?protocol=2&scan_count=200&preview_bytes=100&preview_elements=5&memory_column=false&key_separator=%2F&socket_connect_timeout=7",
      )?.values,
    ).toMatchObject({
      connectTimeoutSecs: "7",
      driverParams: {
        scanCount: "200",
        previewBytes: "100",
        previewElements: "5",
        memoryColumn: false,
        keySeparator: "/",
        protocol: "resp2",
      },
    });
    expect(parseRedisUrl("redis://h?protocol=3")?.values.driverParams).toMatchObject({
      protocol: "resp3",
    });
  });

  it("are not read when they are not Redis URLs or name no valid database", () => {
    for (const bad of [
      "mongodb://h",
      "redis://",
      "redis://:pw@/0",
      "redis://h/x",
      "redis://h/4096",
      "redis://h?protocol=4",
      "redis://h?protocol=toString",
      "redis://h%20x",
    ])
      expect(parseRedisUrl(bad), bad).toBeNull();
  });

  it("write every profile as a URL that reads back to the same profile, without a password", () => {
    const profiles = [
      saved(base()),
      saved(base({ user: "app", port: "6380", database: "3", ssl: "require" })),
      saved(base({ host: "::1", database: "db2", ssl: "verify-full" })),
      saved(base({ ssl: "verify-ca", connectTimeoutSecs: "9" })),
      saved(
        base({
          user: "a b",
          driverParams: {
            ...DEFAULT_PARAMS,
            scanCount: "50",
            previewBytes: "2048",
            previewElements: "9",
            memoryColumn: false,
            keySeparator: "::",
            protocol: "resp3",
          },
        }),
      ),
    ];
    for (const p of profiles) {
      const url = formatRedisUrl(p)!;
      expect(url, url).not.toContain("pw");
      const back = profileOf(url);
      for (const field of ["host", "port", "user", "database", "ssl"] as const)
        expect(back[field], `${url} ${field}`).toEqual(p[field]);
      expect(back.options.driverParams, url).toEqual(p.options.driverParams);
      expect(back.options.connectTimeoutSecs, url).toEqual(p.options.connectTimeoutSecs);
    }
    expect(formatRedisUrl(profiles[1]!)).toMatch(/^rediss:\/\/app:@cache\.example\.com:6380\/3/);
    expect(formatRedisUrl(profiles[2]!)).toMatch(/^rediss:\/\/\[::1\]:6379\/db2\?verify=full/);
  });

  it("write Prefer as plain text, as the backend reads it, and no URL for a bad host", () => {
    const p = saved(base({ ssl: "prefer" as SslMode }));
    expect(formatRedisUrl(p)).toMatch(/^redis:\/\//);
    expect(formatRedisUrl({ ...p, host: "a/b" })).toBeNull();
    expect(formatRedisUrl({ ...p, host: null })).toBeNull();
  });

  it("carry the common query (name, Safe Mode) for every connection URL", () => {
    const url = formatRedisUrl(saved(base({ name: "Cache" })))!;
    expect(new URL(url).searchParams.get("name")).toBe("Cache");
  });
});
