import { describe, expect, it } from "vitest";
import { formToProfileInput, urlToForm } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import { profile } from "@/lib/ipc/mock/fixtures";
import lane from ".";
import { FLAVOR_SCHEMES, type FlavorKind } from "./url";

// The flavors' URLs: PostgreSQL URLs under each engine's schemes, the engine's port when the URL
// has none, and a round trip through the form for every scheme.

const KINDS: FlavorKind[] = ["cockroachdb", "greenplum", "redshift"];
const url = (kind: FlavorKind) => lane.kinds![kind]!.url!;

const flavored = (kind: FlavorKind, over: Partial<ConnectionProfile> = {}): ConnectionProfile => {
  const p = profile({ id: "p1", name: "Prod", driver: "postgres", ...over });
  return { ...p, options: { ...p.options, pgFlavor: kind, ...over.options } };
};

describe("the flavors' URLs", () => {
  it("declare each engine's schemes", () => {
    for (const kind of KINDS) expect(url(kind).schemes).toBe(FLAVOR_SCHEMES[kind]);
    expect(FLAVOR_SCHEMES.cockroachdb[0]).toBe("cockroachdb");
    expect(FLAVOR_SCHEMES.redshift).toEqual(["redshift"]);
  });

  it("read PostgreSQL's fields, the engine and its port", () => {
    const crdb = url("cockroachdb").parse(
      "cockroach://root:secret@crdb.example.com/movr?sslmode=verify-full",
    );
    expect(crdb?.values).toMatchObject({
      driver: "postgres",
      pgFlavor: "cockroachdb",
      host: "crdb.example.com",
      port: "26257",
      user: "root",
      database: "movr",
      ssl: "verify-full",
    });
    expect(crdb?.password).toBe("secret");
    expect(
      url("redshift").parse("redshift://awsuser@cl.abc.us-east-1.redshift.amazonaws.com/dev")
        ?.values.port,
    ).toBe("5439");
    expect(url("greenplum").parse("gpdb://gpadmin@gp:6543/warehouse")?.values).toMatchObject({
      pgFlavor: "greenplum",
      port: "6543",
    });
  });

  it("refuse another engine's scheme and what is no URL", () => {
    expect(url("redshift").parse("cockroachdb://h/db")).toBeNull();
    expect(url("greenplum").parse("greenplum://")).toBeNull();
    expect(url("cockroachdb").parse("not a url")).toBeNull();
  });

  it("write the engine's first scheme, with key authentication over SSH kept", () => {
    const p = flavored("redshift", {
      host: "rs.example.com",
      port: 5439,
      user: "awsuser",
      database: "dev",
    });
    expect(url("redshift").format(p)).toMatch(/^redshift:\/\/awsuser@rs\.example\.com:5439\/dev\?/);
    const ssh = flavored("cockroachdb", {
      ssh: {
        host: "bastion",
        port: 22,
        user: "me",
        auth: "key",
        keyPath: null,
        localPort: null,
        keepaliveSecs: null,
        passwordMode: null,
      },
    });
    const text = url("cockroachdb").format(ssh)!;
    expect(text.startsWith("cockroachdb://")).toBe(true);
    expect(text).not.toContain("+ssh");
    expect(text).toContain("usePrivateKey=true");
  });

  it("round-trip through the form for every scheme", () => {
    for (const kind of KINDS)
      for (const scheme of FLAVOR_SCHEMES[kind]) {
        const p = flavored(kind, {
          host: "db.example.com",
          port: 7000,
          user: "alice",
          database: "sales",
          ssl: "verify-full",
        });
        const written = url(kind)
          .format(p)!
          .replace(/^[a-z]+:/, `${scheme}:`);
        const parsed = url(kind).parse(written)!;
        const back = formToProfileInput(
          urlToForm(parsed, 10),
          null,
          "p1",
          lane.kinds![kind],
        ).profile;
        expect(back, scheme).toMatchObject({
          driver: "postgres",
          host: "db.example.com",
          port: 7000,
          user: "alice",
          database: "sales",
          ssl: "verify-full",
        });
        expect(back.options.pgFlavor, scheme).toBe(kind);
        const again = url(kind).parse(url(kind).format(back)!)!;
        const twice = formToProfileInput(
          urlToForm(again, 10),
          null,
          "p1",
          lane.kinds![kind],
        ).profile;
        expect(url(kind).format(twice), scheme).toBe(url(kind).format(back));
      }
  });
});
