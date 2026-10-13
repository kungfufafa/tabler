import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { ConnectionsFilePreview } from "../bindings/ConnectionsFilePreview";
import type { ExportConnectionsRequest } from "../bindings/ExportConnectionsRequest";
import { profile } from "./fixtures";
import { mockProfiles, mockSecrets } from "./handlers";
import { mockError, mockInvoke, registerMock } from "./transport";

/**
 * Mocks of the connection list's import / export and group commands. Exported files live in
 * memory by path; `/Users/me/Downloads/team.tablerconnections` exists from the start (encrypted,
 * password "tabler", with nested groups and icons) so "Import Connections…" can be tried in the
 * browser. Groups are `{ path, icon }` records, in files and in the `connectionGroups` setting
 * (plain names from older versions read as groups without an icon), like `profiles_io.rs`.
 */

interface MockFileEntry {
  profile: ConnectionProfile;
  password: string | null;
  sshSecrets: boolean;
}

interface MockGroup {
  path: string;
  icon: string | null;
}

interface MockFile {
  password: string | null;
  groups: MockGroup[];
  connections: MockFileEntry[];
}

/** `normalize_group_path`: a "/" with white space on either side separates levels. */
const normalizePath = (raw: string | null | undefined) =>
  (raw ?? "")
    .split(/\s+\/\s*|\s*\/\s+/)
    .map((l) => l.trim())
    .filter(Boolean)
    .join(" / ");

/** The enclosing groups of a canonical path, outermost first, then the path itself. */
const withAncestors = (path: string) =>
  path.split(" / ").map((_, i, levels) => levels.slice(0, i + 1).join(" / "));

function addGroup(list: MockGroup[], raw: string, icon: string | null): void {
  const path = normalizePath(raw);
  if (!path) return;
  const existing = list.find((g) => g.path === path);
  if (!existing) list.push({ path, icon });
  else if (!existing.icon) existing.icon = icon;
}

/** `parse_groups`: records, or plain names from older versions. */
function parseGroups(raw: unknown): MockGroup[] {
  const out: MockGroup[] = [];
  for (const v of Array.isArray(raw) ? raw : []) {
    if (typeof v === "string") addGroup(out, v, null);
    else if (v && typeof v === "object" && typeof (v as MockGroup).path === "string") {
      const icon = (v as MockGroup).icon;
      addGroup(out, (v as MockGroup).path, typeof icon === "string" && icon ? icon : null);
    }
  }
  return out;
}

/** Adds every path of `paths` (with its enclosing groups), icons taken from `icons`. */
function addPaths(list: MockGroup[], paths: readonly string[], icons: readonly MockGroup[]) {
  for (const raw of paths) {
    const path = normalizePath(raw);
    if (!path) continue;
    for (const p of withAncestors(path))
      addGroup(list, p, icons.find((g) => g.path === p)?.icon ?? null);
  }
}

async function storedGroups(): Promise<MockGroup[]> {
  const settings = await mockInvoke<Record<string, unknown>>("get_settings", {});
  return parseGroups(settings[GROUPS_SETTING]);
}

const GROUPS_SETTING = "connectionGroups";
export const DEMO_IMPORT_PATH = "/Users/me/Downloads/team.tablerconnections";
export const DEMO_IMPORT_PASSWORD = "tabler";

const files = new Map<string, MockFile>();

const withPasswordCommand = (p: ConnectionProfile, command: string): ConnectionProfile => ({
  ...p,
  options: { ...p.options, passwordMode: "command", passwordCommand: command },
});

function seedDemoFile(): void {
  files.set(DEMO_IMPORT_PATH, {
    password: DEMO_IMPORT_PASSWORD,
    groups: [
      { path: "Team", icon: "users" },
      { path: "Team / Analytics", icon: "database" },
      { path: "Team / Archive", icon: "archive" },
    ],
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
        // IAM authentication: the preview shows the command; it comes in only when kept.
        profile: withPasswordCommand(
          profile(
            {
              id: "f-warehouse",
              name: "Analytics warehouse",
              driver: "mysql",
              color: "orange",
              group: "Team / Analytics",
              host: "warehouse.example.com",
              user: "analyst",
              database: "events",
              ssl: "require",
            },
            "production",
          ),
          "aws rds generate-db-auth-token --hostname warehouse.example.com --port 3306 --username analyst",
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

/**
 * `drop_password_command`: the profile without its password command; one that got its password
 * from it asks for it on every connect instead.
 */
function withoutPasswordCommand(p: ConnectionProfile): ConnectionProfile {
  const { passwordMode } = p.options;
  return {
    ...p,
    options: {
      ...p.options,
      passwordCommand: null,
      passwordMode: passwordMode === "command" ? "ask" : passwordMode,
    },
  };
}

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

registerMock("export_connections", async ({ request }) => {
  const req = request as ExportConnectionsRequest;
  if (!req.path.trim()) mockError("INVALID_INPUT", "Choose where to save the file");
  const password = req.filePassword ? req.filePassword : null;
  if ((req.includePasswords || req.includeSshSecrets) && !password)
    mockError("INVALID_INPUT", "Set a file password to export passwords and passphrases");
  const connections = [...mockProfiles.values()]
    .filter((p) => req.profileIds.includes(p.id))
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .map((p): MockFileEntry => ({
      // The password command travels like a password (only with them, encrypted).
      profile: req.includePasswords ? p : withoutPasswordCommand(p),
      password: req.includePasswords ? (mockSecrets.get(p.id) ?? null) : null,
      sshSecrets: req.includeSshSecrets && p.ssh !== null,
    }));
  if (connections.length === 0) mockError("INVALID_INPUT", "No connections to export");
  const groups: MockGroup[] = [];
  addPaths(
    groups,
    [...req.groups, ...connections.flatMap((c) => c.profile.group ?? [])],
    await storedGroups(),
  );
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
    groups: file.groups.map((g) => g.path),
    connections: file.connections.map((c) => ({
      profile: c.profile,
      duplicateName: names.some((n) => sameName(n, c.profile.name)),
      hasPassword: c.password !== null,
      hasSshSecrets: c.sshSecrets,
    })),
  };
});

registerMock("import_connections", async ({ path, password, ids, keepPasswordCommands }) => {
  const { file, locked } = openFile(path, password);
  if (locked) mockError("INVALID_INPUT", "Enter the file password to import these connections");
  const wanted = new Set(ids as string[]);
  const keep = new Set(
    Array.isArray(keepPasswordCommands) ? (keepPasswordCommands as string[]) : [],
  );
  let order = Math.max(0, ...[...mockProfiles.values()].map((p) => p.sortOrder));
  const now = new Date().toISOString();
  const imported = file.connections
    .filter((c) => wanted.has(c.profile.id))
    .map((c) => {
      // `plan_import`: a password command comes in only when the user kept it.
      const kept = keep.has(c.profile.id) && !!c.profile.options.passwordCommand?.trim();
      const p: ConnectionProfile = {
        ...(kept ? c.profile : withoutPasswordCommand(c.profile)),
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
  // `merge_imported_groups`: the imported connections' groups, plus the file's empty groups when
  // importing everything, merged by path with their icons.
  const used = imported.flatMap((p) => p.group ?? []);
  if (imported.length === file.connections.length) used.push(...file.groups.map((g) => g.path));
  const known = await storedGroups();
  const merged = known.map((g) => ({ ...g }));
  addPaths(merged, used, file.groups);
  if (JSON.stringify(merged) !== JSON.stringify(known))
    await mockInvoke("set_setting", { key: GROUPS_SETTING, value: merged });
  return imported;
});
