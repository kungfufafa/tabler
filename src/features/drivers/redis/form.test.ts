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
import { bootstrapProblem, REDIS_SSL_MODES } from "./form";
import { databaseIndex, redisFormOf, toRedisParams } from "./params";

// The Redis form: what it saves (only the params that differ from the backend's defaults), what
// it reads back, and its checks next to the schema's, in the words of `entry::validate`.

const ui = lane.kinds!.redis!;

const form = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "redis",
  host: "cache.example.com",
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

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

describe("the Redis form", () => {
  it("starts on 127.0.0.1:6379 without TLS, with the shared rows and on/off TLS modes", () => {
    expect(ui.rows).toEqual(["hostPort", "user", "secret", "database", "ssl"]);
    expect(ui.labels).toEqual({ user: "ACL user", database: "DB index" });
    expect(ui.defaults!()).toEqual({
      host: "127.0.0.1",
      port: "6379",
      ssl: "disable",
      driverParams: {
        scanCount: "",
        previewBytes: "",
        previewElements: "",
        memoryColumn: true,
        keySeparator: "",
        protocol: "auto",
      },
    });
    // No Prefer: the backend reads it as plain text (Redis has no TLS negotiation).
    expect(REDIS_SSL_MODES.map(([m]) => m)).toEqual([
      "disable",
      "require",
      "verify-ca",
      "verify-full",
    ]);
    expect(DESCRIPTORS.redis.defaultPort).toBe(6379);
    expect(ui.url!.schemes).toEqual(DESCRIPTORS.redis.urlSchemes);
  });

  it("saves only the params that differ from the defaults, and reads them back", () => {
    expect(saved(form()).options.driverParams).toBeNull();
    const p = saved(
      form({
        database: "3",
        user: "app",
        ssl: "verify-full",
        driverParams: params({
          scanCount: "500",
          previewBytes: "4096",
          previewElements: "20",
          memoryColumn: false,
          keySeparator: "/",
          protocol: "resp2",
        }),
      }),
    );
    expect(p.options.driverParams).toEqual({
      scanCount: 500,
      previewBytes: 4096,
      previewElements: 20,
      memoryColumn: false,
      keySeparator: "/",
      protocol: "resp2",
    });
    expect(p.port).toBe(6379);
    expect(p.database).toBe("3");
    const back = profileToForm(p, ui);
    expect(back.driverParams).toEqual({
      scanCount: "500",
      previewBytes: "4096",
      previewElements: "20",
      memoryColumn: false,
      keySeparator: "/",
      protocol: "resp2",
    });
    expect(saved({ ...form(), ...back, password: "" }).options.driverParams).toEqual(
      p.options.driverParams,
    );
    // The default separator is not saved.
    expect(toRedisParams(redisFormOf({ keySeparator: ":" }))).toBeNull();
  });

  it("reads the params leniently: the wrong type counts as unset", () => {
    expect(
      redisFormOf({ scanCount: true, protocol: "resp4", memoryColumn: "no", keySeparator: 1 }),
    ).toEqual({
      scanCount: "",
      previewBytes: "",
      previewElements: "",
      memoryColumn: true,
      keySeparator: "",
      protocol: "auto",
    });
  });

  it("checks the settings, the DB index and the bootstrap commands as the backend does", () => {
    expect(issues(form())).toEqual([]);
    expect(
      issues(
        form({
          database: "db4096",
          driverParams: params({
            scanCount: "0",
            previewBytes: "x",
            previewElements: "10001",
            keySeparator: "123456789",
          }),
        }),
      ),
    ).toEqual([
      "driverParams.scanCount: Between 1 and 100,000",
      "driverParams.previewBytes: Between 1 and 16,777,216",
      "driverParams.previewElements: Between 1 and 10,000",
      "driverParams.keySeparator: 1 to 8 characters",
      "database: A number from 0 to 4095 (or db0, db1, …)",
    ]);
    // Code points, as Rust counts `chars()`.
    expect(issues(form({ driverParams: params({ keySeparator: "😀".repeat(8) }) }))).toEqual([]);
    for (const [text, n] of [
      ["", 0],
      ["3", 3],
      [" 15 ", 15],
      ["db2", 2],
      ["+7", 7],
    ] as const)
      expect(databaseIndex(text), text).toBe(n);
    for (const bad of ["-1", "x", "4096", "db", "1.5"]) expect(databaseIndex(bad), bad).toBeNull();
    expect(issues(form({ startupSql: "CLIENT SETNAME reports\nCONFIG GET maxmemory" }))).toEqual(
      [],
    );
    expect(issues(form({ startupSql: "SELECT 2" }))).toEqual([
      "startupSql: Bootstrap command “SELECT 2”: the DB index field picks the database, and a transaction cannot span connections",
    ]);
    expect(bootstrapProblem("MONITOR", false)).toContain("not supported in the console");
    expect(bootstrapProblem('GET "a', false)).toContain("unbalanced quotes");
    expect(bootstrapProblem("CLIENT SETNAME x", true)).toBe(
      "Bootstrap command “CLIENT SETNAME x”: it writes, and the connection is read-only",
    );
    expect(bootstrapProblem("PING; ECHO hi", true)).toBeNull();
  });

  it("sums a profile up as user@host:port/db", () => {
    expect(ui.summary!(saved(form()))).toBe("cache.example.com:6379/db0");
    expect(ui.summary!(saved(form({ user: "app", database: "db5", ssl: "require" })))).toBe(
      "app@cache.example.com:6379/db5 (TLS)",
    );
  });
});
