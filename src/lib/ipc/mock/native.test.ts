import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { ipc, type ConnectionProfile, type ProfileInput } from "@/lib/ipc";
import { profile } from "./fixtures";

const input = (p: ConnectionProfile): ProfileInput => ({
  profile: p,
  password: null,
  sshPassword: null,
  sshPassphrase: null,
  clearPassword: false,
});

describe("mock driver calls", () => {
  let c = "";
  beforeAll(async () => {
    c = (await ipc.connections.connect("p-local-pg")).connectionId;
  });
  afterAll(async () => {
    await ipc.connections.disconnect(c);
  });

  it("refuses a core connection's calls as unknown, as the backend does", async () => {
    await expect(ipc.sessionCall(c, "stats")).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: PostgreSQL: unknown call “stats”",
    });
    await expect(ipc.sessionCall("gone", "stats")).rejects.toMatchObject({
      code: "NOT_CONNECTED",
    });
  });

  it("refuses the form's calls of a core driver, and of a database this build lacks", async () => {
    const mysql = profile({ id: "p-util-my", name: "My", driver: "mysql" });
    await expect(ipc.driverUtil(input(mysql), "probe")).rejects.toMatchObject({
      message: "unsupported: MySQL: unknown call “probe”",
    });
    const redis = profile({ id: "p-util-redis", name: "Cache", driver: "redis" });
    await expect(ipc.driverUtil(input(redis), "probe")).rejects.toMatchObject({
      message: "unsupported: Redis is not built into this Tabler",
    });
  });
});
