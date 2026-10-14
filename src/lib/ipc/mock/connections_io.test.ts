import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockConnectionFiles } from "./connections_io";
import { resetMockSettings } from "./settings";

const GROUPS = "connectionGroups";
const PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

beforeAll(async () => {
  await import("@/lib/ipc/mock");
});
beforeEach(() => {
  resetMockSettings();
  resetMockConnectionFiles();
});

const request = (path: string, groups: string[] = []) => ({
  profileIds: ["p-prod-pg"],
  groups,
  includePasswords: false,
  includeSshSecrets: false,
  filePassword: null,
  path,
});

/** The profile of a fixture in a group (exports take its group along). */
async function inGroup(group: string) {
  await ipc.profiles.moveToGroup(["p-prod-pg"], group);
}

describe("mock group images (like profiles_io.rs)", () => {
  it("exports the images of the groups, and imports them into an app that has none", async () => {
    await inGroup("Team");
    await ipc.settings.set(GROUPS, [
      { path: "Team", icon: "users", image: PNG },
      { path: "Empty", icon: null, image: PNG },
    ]);
    await ipc.profiles.exportConnections(request("/tmp/pictures.tablerconnections", ["Empty"]));
    // The preview lists the group paths of the file.
    const preview = await ipc.profiles.inspectConnectionsFile("/tmp/pictures.tablerconnections");
    expect(preview.groups).toEqual(["Empty", "Team"]);

    // An app without those groups (a fresh settings store) takes them with their images.
    resetMockSettings();
    await ipc.profiles.importConnections("/tmp/pictures.tablerconnections", null, ["p-prod-pg"]);
    // (The imported connections' groups first, then the file's empty ones, like the backend.)
    expect(await ipc.settings.get().then((s) => s[GROUPS])).toEqual([
      { path: "Team", icon: "users", image: PNG },
      { path: "Empty", icon: null, image: PNG },
    ]);
  });

  it("never erases a known image, and fills a missing one", async () => {
    await inGroup("Team");
    await ipc.settings.set(GROUPS, [{ path: "Team", icon: "users", image: PNG }]);
    await ipc.profiles.exportConnections(request("/tmp/keep.tablerconnections"));

    // The app has the group with another image: it stays.
    await ipc.settings.set(GROUPS, [
      { path: "Team", icon: "rocket", image: "data:image/png;base64,KEEP" },
    ]);
    await ipc.profiles.importConnections("/tmp/keep.tablerconnections", null, ["p-prod-pg"]);
    expect(await ipc.settings.get().then((s) => s[GROUPS])).toEqual([
      { path: "Team", icon: "rocket", image: "data:image/png;base64,KEEP" },
    ]);

    // The app has the group without an image: the file's fills it (its icon stays).
    await ipc.settings.set(GROUPS, [{ path: "Team", icon: "rocket" }]);
    await ipc.profiles.importConnections("/tmp/keep.tablerconnections", null, ["p-prod-pg"]);
    expect(await ipc.settings.get().then((s) => s[GROUPS])).toEqual([
      { path: "Team", icon: "rocket", image: PNG },
    ]);
  });

  it("a file group without an image does not erase the stored one", async () => {
    await inGroup("Team");
    await ipc.settings.set(GROUPS, [{ path: "Team", icon: "users" }]);
    await ipc.profiles.exportConnections(request("/tmp/plain.tablerconnections"));
    await ipc.settings.set(GROUPS, [{ path: "Team", icon: null, image: PNG }]);
    await ipc.profiles.importConnections("/tmp/plain.tablerconnections", null, ["p-prod-pg"]);
    expect(await ipc.settings.get().then((s) => s[GROUPS])).toEqual([
      { path: "Team", icon: "users", image: PNG },
    ]);
  });

  it("drops an image that is not a data:image URI, or too long, when it reads the setting", async () => {
    await inGroup("Team");
    await ipc.settings.set(GROUPS, [
      { path: "Team", icon: "users", image: "javascript:alert(1)" },
      { path: "Remote", image: "https://example.com/logo.png" },
      { path: "Huge", image: `data:image/png;base64,${"A".repeat(24 * 1024)}` },
    ]);
    await ipc.profiles.exportConnections(request("/tmp/bad.tablerconnections", ["Remote", "Huge"]));
    resetMockSettings();
    await ipc.profiles.importConnections("/tmp/bad.tablerconnections", null, ["p-prod-pg"]);
    const groups = (await ipc.settings.get().then((s) => s[GROUPS])) as Array<
      Record<string, unknown>
    >;
    expect(groups.map((g) => g.path)).toEqual(["Team", "Remote", "Huge"]);
    expect(groups.every((g) => !("image" in g))).toBe(true);
    expect(groups.find((g) => g.path === "Team")?.icon).toBe("users");
  });
});
