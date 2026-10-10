import { describe, expect, it } from "vitest";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { connectionFormSchema, DEFAULT_FORM, formToProfileInput, profileToForm } from "./schema";

describe("connection form schema", () => {
  it("requires host and user for server drivers, a path for sqlite", () => {
    const r1 = connectionFormSchema.safeParse({ ...DEFAULT_FORM, name: "x", host: "", user: "" });
    expect(r1.success).toBe(false);
    const paths = r1.success ? [] : r1.error.issues.map((i) => i.path.join("."));
    expect(paths).toEqual(expect.arrayContaining(["host", "user"]));
    const r2 = connectionFormSchema.safeParse({ ...DEFAULT_FORM, name: "x", driver: "sqlite" });
    expect(r2.success).toBe(false);
    const r3 = connectionFormSchema.safeParse({
      ...DEFAULT_FORM,
      name: "x",
      driver: "sqlite",
      sqlitePath: "/tmp/a.db",
    });
    expect(r3.success).toBe(true);
    const r4 = connectionFormSchema.safeParse({
      ...DEFAULT_FORM,
      name: "x",
      user: "u",
      port: "99999",
    });
    expect(r4.success).toBe(false);
  });

  it("round-trips a profile through the form", () => {
    const pg = SEED_PROFILES[0]!;
    const values = profileToForm(pg);
    expect(values.driver).toBe("postgres");
    expect(values.port).toBe("5432");
    const input = formToProfileInput({ ...values, password: "pw" }, pg, pg.id);
    expect(input.profile.id).toBe(pg.id);
    expect(input.profile.host).toBe("127.0.0.1");
    expect(input.profile.port).toBe(5432);
    expect(input.profile.ssh).toBeNull();
    expect(input.password).toBe("pw");
    expect(input.clearPassword).toBe(false);
    const sqlite = formToProfileInput(
      { ...DEFAULT_FORM, name: "s", driver: "sqlite", sqlitePath: "/x.db" },
      null,
      "id-1",
    );
    expect(sqlite.profile.host).toBeNull();
    expect(sqlite.profile.sqlitePath).toBe("/x.db");
    expect(sqlite.password).toBeNull();
    const ssh = formToProfileInput(
      {
        ...DEFAULT_FORM,
        name: "s",
        user: "u",
        sshEnabled: true,
        sshHost: "b",
        sshUser: "me",
        sshAuth: "password",
      },
      null,
      "id-2",
    );
    expect(ssh.profile.ssh).toEqual({
      host: "b",
      port: 22,
      user: "me",
      auth: "password",
      keyPath: null,
      localPort: null,
      keepaliveSecs: null,
    });
  });
});
