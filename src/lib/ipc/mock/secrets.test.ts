import { afterEach, describe, expect, it } from "vitest";
import { ipc, type ConnectionProfile, type ProfileInput } from "@/lib/ipc";
import { profile } from "./fixtures";
import { resetMockPasscode } from "./security";

const input = (p: ConnectionProfile, patch: Partial<ProfileInput> = {}): ProfileInput => ({
  profile: p,
  password: null,
  sshPassword: null,
  sshPassphrase: null,
  clearPassword: false,
  ...patch,
});

afterEach(() => resetMockPasscode());

describe("mock second secrets", () => {
  it("store, keep, copy and remove a driver's second secrets like the backend", async () => {
    const aws = profile({ id: "p-mock-aws", name: "Orders", driver: "dynamodb" });
    // DynamoDB takes a session token, no key passphrase.
    await ipc.profiles.save(input(aws, { sessionToken: "tok", keyPassphrase: "not taken" }));
    expect(await ipc.profiles.secretStatus(aws.id)).toMatchObject({
      hasSessionToken: true,
      hasKeyPassphrase: false,
    });
    // Untouched: kept.
    await ipc.profiles.save(input(aws));
    expect((await ipc.profiles.secretStatus(aws.id)).hasSessionToken).toBe(true);
    const copy = await ipc.profiles.duplicate(aws.id);
    expect((await ipc.profiles.secretStatus(copy.id)).hasSessionToken).toBe(true);
    // "" removes it; deleting the profile takes it along.
    await ipc.profiles.save(input(aws, { sessionToken: "" }));
    expect((await ipc.profiles.secretStatus(aws.id)).hasSessionToken).toBe(false);
    await ipc.profiles.delete(copy.id);
    expect((await ipc.profiles.secretStatus(copy.id)).hasSessionToken).toBe(false);
    await ipc.profiles.delete(aws.id);
  });
});

describe("mock Safe Mode check", () => {
  it("takes a token connection's passcode when one is set, else its name", async () => {
    const d1 = profile({ id: "p-mock-d1", name: "Edge", driver: "d1" });
    await ipc.profiles.save(input(d1, { password: "api-token" }));
    expect(await ipc.safety.verifyPassword(d1.id, " Edge ")).toBe(true);
    expect(await ipc.safety.verifyPassword(d1.id, "api-token")).toBe(false);
    await ipc.security.setPasscode(null, "4321");
    expect(await ipc.safety.verifyPassword(d1.id, "4321")).toBe(true);
    expect(await ipc.safety.verifyPassword(d1.id, "Edge")).toBe(false);
    // The core profiles keep their password.
    expect(await ipc.safety.verifyPassword("p-local-pg", "secret")).toBe(true);
    expect(await ipc.safety.verifyPassword("p-local-pg", "4321")).toBe(false);
    await ipc.profiles.delete(d1.id);
  });
});
