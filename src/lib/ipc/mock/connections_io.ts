import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { ConnectionsFilePreview } from "../bindings/ConnectionsFilePreview";
import type { ExportConnectionsRequest } from "../bindings/ExportConnectionsRequest";
import { profile } from "./fixtures";
import { mockProfiles, mockSecrets } from "./handlers";
import { mockError, mockInvoke, registerMock } from "./transport";

/**
 * Mocks of the connection list's import / export and group commands. Exported files live in
 * memory by path; `/Users/me/Downloads/team.tablerconnections` exists from the start (encrypted,
 * password "tabler") so "Import Connections…" can be tried in the browser.
 */

interface MockFileEntry {
  profile: ConnectionProfile;
  password: string | null;
  sshSecrets: boolean;
}

interface MockFile {
  password: string | null;
  groups: string[];
  connections: MockFileEntry[];
}

const GROUPS_SETTING = "connectionGroups";
export const DEMO_IMPORT_PATH = "/Users/me/Downloads/team.tablerconnections";
export const DEMO_IMPORT_PASSWORD = "tabler";

const files = new Map<string, MockFile>();

function seedDemoFile(): void {
  files.set(DEMO_IMPORT_PATH, {
    password: DEMO_IMPORT_PASSWORD,
    groups: ["Team", "Archive"],
    connections: [
      {
        profile: profile(
          {
            id: "f-team-pg",
            name: "Team Postgres",
            driver: "postgres",
            color: "teal",
            group: "Team",
            host: "pg.team.example.com",
            user: "app",
            database: "app",
          },
          "development",
        ),
        password: "team-secret",
        sshSecrets: false,
      },
      {
        profile: profile(
          {
            id: "f-warehouse",
            name: "Analytics warehouse",
            driver: "mysql",
            color: "orange",
            group: "Team",
            host: "warehouse.example.com",
            user: "analyst",
            database: "events",
          },
          "production",
        ),
        password: null,
        sshSecrets: false,
      },
      {
        profile: profile(
          { id: "f-local-pg", name: "Local Postgres", driver: "postgres", color: "blue" },
          "local",
        ),
        password: "secret",
        sshSecrets: false,
      },
    ],
  });
}
seedDemoFile();

/** Test helper: forget exported files and restore the demo file. */
export function resetMockConnectionFiles(): void {
  files.clear();
  seedDemoFile();
}

const wrongPassword = () =>
  mockError("INVALID_INPUT", "The file password is wrong, or the file was modified");

function openFile(path: unknown, password: unknown): { file: MockFile; locked: boolean } {
  const file = typeof path === "string" ? files.get(path) : undefined;
  if (!file) mockError("IO", `${String(path)}: No such file or directory (os error 2)`);
  const typed = typeof password === "string" && password !== "" ? password : null;
  if (file.password === null) return { file, locked: false };
  if (typed === null) return { file, locked: true };
  if (typed !== file.password) wrongPassword();
  return { file, locked: false };
}

const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

registerMock("move_profiles_to_group", ({ ids, group }) => {
  const g = typeof group === "string" && group.trim() !== "" ? group.trim() : null;
  const out: ConnectionProfile[] = [];
  for (const id of ids as string[]) {
    const p = mockProfiles.get(id);
    if (!p) continue;
    const moved = { ...p, group: g, updatedAt: new Date().toISOString() };
    mockProfiles.set(id, moved);
    out.push(moved);
  }
  return out;
});

registerMock("export_connections", ({ request }) => {
  const req = request as ExportConnectionsRequest;
  if (!req.path.trim()) mockError("INVALID_INPUT", "Choose where to save the file");
  const password = req.filePassword ? req.filePassword : null;
  if ((req.includePasswords || req.includeSshSecrets) && !password)
    mockError("INVALID_INPUT", "Set a file password to export passwords and passphrases");
  const connections = [...mockProfiles.values()]
    .filter((p) => req.profileIds.includes(p.id))
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .map((p): MockFileEntry => ({
      profile: p,
      password: req.includePasswords ? (mockSecrets.get(p.id) ?? null) : null,
      sshSecrets: req.includeSshSecrets && p.ssh !== null,
    }));
  if (connections.length === 0) mockError("INVALID_INPUT", "No connections to export");
  const groups = [
    ...new Set([...req.groups, ...connections.flatMap((c) => c.profile.group ?? [])]),
  ];
  files.set(req.path, { password, groups, connections });
  return connections.length;
});

registerMock("inspect_connections_file", ({ path, password }): ConnectionsFilePreview => {
  const { file, locked } = openFile(path, password);
  if (locked) return { encrypted: true, needsPassword: true, groups: [], connections: [] };
  const names = [...mockProfiles.values()].map((p) => p.name);
  return {
    encrypted: file.password !== null,
    needsPassword: false,
    groups: file.groups,
    connections: file.connections.map((c) => ({
      profile: c.profile,
      duplicateName: names.some((n) => sameName(n, c.profile.name)),
      hasPassword: c.password !== null,
      hasSshSecrets: c.sshSecrets,
    })),
  };
});

registerMock("import_connections", async ({ path, password, ids }) => {
  const { file, locked } = openFile(path, password);
  if (locked) mockError("INVALID_INPUT", "Enter the file password to import these connections");
  const wanted = new Set(ids as string[]);
  let order = Math.max(0, ...[...mockProfiles.values()].map((p) => p.sortOrder));
  const now = new Date().toISOString();
  const imported = file.connections
    .filter((c) => wanted.has(c.profile.id))
    .map((c) => {
      const p: ConnectionProfile = {
        ...c.profile,
        id: crypto.randomUUID(),
        sortOrder: ++order,
        createdAt: now,
        updatedAt: now,
        lastConnectedAt: null,
      };
      mockProfiles.set(p.id, p);
      if (c.password !== null) mockSecrets.set(p.id, c.password);
      return p;
    });
  const extra = imported.flatMap((p) => p.group ?? []);
  if (imported.length === file.connections.length) extra.push(...file.groups);
  const settings = await mockInvoke<Record<string, unknown>>("get_settings", {});
  const stored = Array.isArray(settings[GROUPS_SETTING])
    ? (settings[GROUPS_SETTING] as unknown[]).filter((g): g is string => typeof g === "string")
    : [];
  const merged = [...new Set([...stored, ...extra.map((g) => g.trim()).filter(Boolean)])];
  if (merged.length !== stored.length)
    await mockInvoke("set_setting", { key: GROUPS_SETTING, value: merged });
  return imported;
});
