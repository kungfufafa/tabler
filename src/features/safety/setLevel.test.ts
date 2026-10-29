import { describe, expect, it } from "vitest";
import type { ConnectionProfile, ProfileInput, SafeModeLevel } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { protectedChanges } from "./setLevel";

const PG = SEED_PROFILES.find((p) => p.id === "p-local-pg")!;

function saved(safeMode: SafeModeLevel, options: Partial<ConnectionProfile["options"]> = {}) {
  return { ...PG, options: { ...PG.options, safeMode, readOnly: true, ...options } };
}

function input(
  profile: ConnectionProfile,
  options: Partial<ConnectionProfile["options"]> = {},
  secrets: Partial<Omit<ProfileInput, "profile">> = {},
): ProfileInput {
  return {
    profile: { ...profile, options: { ...profile.options, ...options } },
    password: null,
    sshPassword: null,
    sshPassphrase: null,
    clearPassword: false,
    ...secrets,
  };
}

describe("protectedChanges", () => {
  it("lists what a save weakens on a Safe Mode 1 / 2 connection", () => {
    const safe = saved("safe");
    expect(protectedChanges(safe, input(safe))).toEqual([]);
    // Moving up or renaming asks nothing.
    expect(protectedChanges(safe, input({ ...safe, name: "Renamed" }))).toEqual([]);
    expect(
      protectedChanges(
        safe,
        input(safe, { safeMode: "alert", readOnly: false, passwordMode: "ask" }),
      ),
    ).toEqual([
      "Switching from Safe Mode 1 to Alert Mode 1.",
      "Turning off Read-only.",
      "Changing the password storage from Store in keychain to Ask every time.",
    ]);
    expect(protectedChanges(safe, input(safe, {}, { password: "new" }))).toEqual([
      "Saving a new password.",
    ]);
    expect(protectedChanges(safe, input(safe, {}, { clearPassword: true }))).toEqual([
      "Removing the saved password.",
    ]);
    const safe2 = saved("safeNonSelect", { passwordMode: "ask" });
    expect(protectedChanges(safe2, input(safe2, { passwordMode: "none" }))).toEqual([
      "Changing the password storage from Ask every time to No password.",
    ]);
  });

  it("names the driver's secret and its second secrets", () => {
    // A SQLite profile's password is its encryption key, but it is still called the password.
    const lite = { ...saved("safe"), driver: "sqlite" as const };
    expect(protectedChanges(lite, input(lite, {}, { password: "k" }))).toEqual([
      "Saving a new password.",
    ]);
    const aws = { ...saved("safe"), driver: "dynamodb" as const };
    expect(
      protectedChanges(
        aws,
        input(aws, {}, { password: "s", sessionToken: "t", keyPassphrase: "not taken" }),
      ),
    ).toEqual(["Saving a new secret access key.", "Saving a new session token."]);
    expect(protectedChanges(aws, input(aws, {}, { sessionToken: "" }))).toEqual([
      "Removing the saved session token.",
    ]);
    // Untouched (absent or null): nothing.
    expect(protectedChanges(aws, input(aws, {}, { sessionToken: null }))).toEqual([]);
    const snow = { ...saved("safe"), driver: "snowflake" as const };
    expect(
      protectedChanges(snow, input(snow, {}, { clearPassword: true, keyPassphrase: "pp" })),
    ).toEqual(["Removing the saved password or token.", "Saving a new key passphrase."]);
  });

  it("asks nothing below Safe Mode", () => {
    for (const level of ["silent", "alert", "alertNonSelect"] as const) {
      const p = saved(level);
      expect(
        protectedChanges(p, input(p, { safeMode: "silent", readOnly: false }, { password: "x" })),
      ).toEqual([]);
    }
  });
});
