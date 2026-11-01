import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import lane from ".";
import { parseCassandraUrl } from "./url";

// `cassandra://` URLs read into the form (a password into the secret), and a profile written as
// a URL that reads back to the same profile.

const ui = lane.kinds!.cassandra!;
const url = ui.url!;

/** What a URL makes of a new form, then saved: the profile and the secret. */
function saved(text: string): { profile: ConnectionProfile; password: string | null } {
  const parsed = url.parse(text);
  if (!parsed) throw new Error(`not read: ${text}`);
  const values: ConnectionFormValues = {
    ...DEFAULT_FORM,
    ...ui.defaults!(),
    driver: "cassandra",
    name: "n",
    ...parsed.values,
    driverParams: { ...ui.defaults!().driverParams, ...parsed.values.driverParams },
  };
  return {
    profile: formToProfileInput(values, null, "id-1", ui).profile,
    password: parsed.password,
  };
}

describe("cassandra:// URLs", () => {
  it("are the kind's only scheme, as the descriptor says", () => {
    expect(url.schemes).toEqual(["cassandra"]);
  });

  it("read the user, password, host, port and keyspace", () => {
    const p = parseCassandraUrl("cassandra://app:p%40ss@db.example.com:9142/Shop%20Ks");
    expect(p?.password).toBe("p@ss");
    expect(p?.values).toMatchObject({
      host: "db.example.com",
      port: "9142",
      user: "app",
      database: "Shop Ks",
      ssl: "disable",
      driverParams: { consistency: "LOCAL_ONE", singleNode: false },
    });
    expect(parseCassandraUrl("CASSANDRA://[::1]")?.values).toMatchObject({
      host: "::1",
      port: "",
      user: "",
      database: "",
    });
    expect(parseCassandraUrl("cassandra://u@h")?.password).toBeNull();
  });

  it("read TLS and the settings under their own names and other tools' spellings", () => {
    const p = parseCassandraUrl(
      "cassandra://h/ks?sslmode=verify-ca&consistency=local_quorum&serial_consistency=SERIAL&localDC=dc2&requestTimeout=12&fetchSize=250&compression=LZ4&singleNode=1&connectTimeout=7",
    );
    expect(p?.values).toMatchObject({
      ssl: "verify-ca",
      connectTimeoutSecs: "7",
      driverParams: {
        consistency: "LOCAL_QUORUM",
        serialConsistency: "SERIAL",
        localDatacenter: "dc2",
        requestTimeoutSecs: "12",
        pageSize: "250",
        compression: "lz4",
        singleNode: true,
      },
    });
    expect(parseCassandraUrl("cassandra://h?ssl=true")?.values.ssl).toBe("require");
    expect(parseCassandraUrl("cassandra://h?ssl=false")?.values.ssl).toBe("disable");
    // CQL has no negotiation: Prefer is Disable.
    expect(parseCassandraUrl("cassandra://h?sslmode=prefer")?.values.ssl).toBe("disable");
    // `ssl`/`tls` also take a mode's name.
    for (const [q, mode] of [
      ["ssl=required", "require"],
      ["tls=require", "require"],
      ["TLS=Verify-Full", "verify-full"],
      ["ssl=verify_ca", "verify-ca"],
      ["ssl=disabled", "disable"],
    ])
      expect(parseCassandraUrl(`cassandra://h/ks?${q}`)?.values.ssl, q).toBe(mode);
  });

  it("refuse a TLS value they cannot read rather than connect in plaintext", () => {
    for (const q of [
      "ssl=maybe",
      "ssl=enabled",
      "tls=",
      "ssl=constructor",
      "sslmode=toString",
      "tls=sometimes",
    ])
      expect(parseCassandraUrl(`cassandra://u:pw@h/ks?${q}`), q).toBeNull();
  });

  it("leave out values the backend would refuse, and refuse what is no URL of theirs", () => {
    const p = parseCassandraUrl("cassandra://h?requestTimeout=0&pageSize=99999&consistency=SOME");
    expect(p?.values.driverParams).toMatchObject({
      requestTimeoutSecs: "",
      pageSize: "",
      consistency: "LOCAL_ONE",
    });
    for (const bad of [
      "postgres://h/db",
      "cassandra://",
      "cassandra://u:%zz@h",
      "cassandra://h/a/b",
      "cassandra://h?sslmode=sometimes",
      // A contact point holds none of these; written back they would read as something else.
      "cassandra://h%20/ks",
      "cassandra://h%40x/ks",
      "cassandra://h%2Fy/ks",
      "cassandra://h%3Fz",
      "cassandra://h%23z",
    ])
      expect(parseCassandraUrl(bad), bad).toBeNull();
  });

  it("write a profile that reads back the same, without its password", () => {
    for (const text of [
      "cassandra://db.example.com",
      "cassandra://app@db.example.com:9042/shop",
      "cassandra://a%40b@[2001:db8::1]:9142/My%20Ks?sslmode=verify-full",
      "cassandra://h/ks?sslmode=require&consistency=EACH_QUORUM&serialConsistency=SERIAL&localDatacenter=dc1&requestTimeout=60&pageSize=1000&compression=snappy&singleNode=true&connectTimeout=9",
    ]) {
      const first = saved(text);
      const written = url.format(first.profile);
      expect(written, text).toBe(text);
      expect(saved(written!).profile, text).toEqual(first.profile);
    }
    const withPassword = saved("cassandra://app:secret@h/ks");
    expect(withPassword.password).toBe("secret");
    expect(url.format(withPassword.profile)).toBe("cassandra://app@h/ks");
  });

  it("write nothing for a profile without a host, or with one no URL can carry", () => {
    const p = saved("cassandra://h").profile;
    expect(url.format({ ...p, host: null })).toBeNull();
    for (const host of ["h@x/y?z", "h ", "a#b"])
      expect(url.format({ ...p, host }), host).toBeNull();
  });
});
