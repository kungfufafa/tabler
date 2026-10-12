import type {
  Driver,
  ObjectPrivileges,
  ResourceLimits,
  RoleAttributes,
  UserChange,
  UserRef,
  UserState,
  UserSummary,
} from "@/lib/ipc";

/** MySQL `ON *.*` privileges (the Global Privileges tab), in the backend's order. */
export const MYSQL_GLOBAL_PRIVILEGES = [
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "CREATE",
  "DROP",
  "RELOAD",
  "PROCESS",
  "FILE",
  "REFERENCES",
  "INDEX",
  "ALTER",
  "SHOW DATABASES",
  "SUPER",
  "CREATE TEMPORARY TABLES",
  "LOCK TABLES",
  "EXECUTE",
  "REPLICATION SLAVE",
  "REPLICATION CLIENT",
  "CREATE VIEW",
  "SHOW VIEW",
  "CREATE ROUTINE",
  "ALTER ROUTINE",
  "CREATE USER",
  "EVENT",
  "TRIGGER",
] as const;

/** MySQL `ON db.*` privileges. */
export const MYSQL_DB_PRIVILEGES = [
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "CREATE",
  "DROP",
  "REFERENCES",
  "INDEX",
  "ALTER",
  "CREATE TEMPORARY TABLES",
  "LOCK TABLES",
  "EXECUTE",
  "CREATE VIEW",
  "SHOW VIEW",
  "CREATE ROUTINE",
  "ALTER ROUTINE",
  "EVENT",
  "TRIGGER",
] as const;

export const PG_DATABASE_PRIVILEGES = ["CONNECT", "CREATE", "TEMPORARY"] as const;
export const PG_SCHEMA_PRIVILEGES = ["USAGE", "CREATE"] as const;
export const PG_TABLE_PRIVILEGES = ["SELECT", "INSERT", "UPDATE", "DELETE"] as const;

/** What a PG role gets from `CREATE ROLE` without options (plus LOGIN: a user). */
export const DEFAULT_ROLE_ATTRIBUTES: RoleAttributes = {
  superuser: false,
  createDb: false,
  createRole: false,
  login: true,
  replication: false,
  inherit: true,
  connectionLimit: -1,
  validUntil: null,
};

export const NO_LIMITS: ResourceLimits = {
  maxQueriesPerHour: 0,
  maxUpdatesPerHour: 0,
  maxConnectionsPerHour: 0,
  maxUserConnections: 0,
};

/** One account of the list on the left, and the key that tells them apart. */
export function userKey(u: { name: string; host: string | null }): string {
  return u.host === null ? u.name : `${u.name}@${u.host}`;
}

/** `'app'@'%'` (MySQL) / `app` (PG). */
export function userLabel(u: { name: string; host: string | null }): string {
  return u.host === null ? u.name : `'${u.name}'@'${u.host}'`;
}

export function userRef(u: UserSummary | UserState): UserRef {
  return { name: u.name, host: u.host };
}

/** A new account's starting point: MySQL `'new_user'@'%'` without privileges, PG LOGIN role. */
export function newUserState(driver: Driver): UserState {
  return {
    name: "",
    host: driver === "mysql" ? "%" : null,
    globalPrivileges: [],
    limits: driver === "mysql" ? NO_LIMITS : null,
    attributes: driver === "postgres" ? DEFAULT_ROLE_ATTRIBUTES : null,
    databasePrivileges: [],
    schemaPrivileges: [],
    tablePrivileges: [],
    partialTablePrivileges: [],
  };
}

/** The editor's working copy. `original: null` = a new account; `password: null` = unchanged. */
export interface UserDraft {
  original: UserState | null;
  state: UserState;
  password: string | null;
}

export function draftOf(original: UserState): UserDraft {
  return { original, state: original, password: null };
}

export function newDraft(driver: Driver): UserDraft {
  return { original: null, state: newUserState(driver), password: null };
}

/** Privileges of `object` in `list`. */
export function privilegesOn(list: readonly ObjectPrivileges[], object: string): string[] {
  return list.find((p) => p.object === object)?.privileges ?? [];
}

/** `list` with `privilege` on `object` turned on / off; `order` keeps the canonical order. */
export function setPrivilege(
  list: readonly ObjectPrivileges[],
  object: string,
  privilege: string,
  on: boolean,
  order: readonly string[],
): ObjectPrivileges[] {
  const current = new Set(privilegesOn(list, object));
  if (on) current.add(privilege);
  else current.delete(privilege);
  return setPrivileges(list, object, [...current], order);
}

/** `list` with `object`'s privileges replaced (an empty set removes the entry). */
export function setPrivileges(
  list: readonly ObjectPrivileges[],
  object: string,
  privileges: readonly string[],
  order: readonly string[],
): ObjectPrivileges[] {
  const sorted = order.filter((p) => privileges.includes(p));
  const rest = list.filter((p) => p.object !== object);
  const next = sorted.length > 0 ? [...rest, { object, privileges: sorted }] : rest;
  return next.sort((a, b) => a.object.localeCompare(b.object));
}

/** Tri-state of a PG table privilege on a schema's tables. */
export type TablePrivilegeState = "all" | "some" | "none";

export function tablePrivilegeState(
  state: UserState,
  schema: string,
  privilege: string,
): TablePrivilegeState {
  if (privilegesOn(state.tablePrivileges, schema).includes(privilege)) return "all";
  if (privilegesOn(state.partialTablePrivileges, schema).includes(privilege)) return "some";
  return "none";
}

/** Grants (`all`) or revokes (`none`) a privilege on every table of `schema`. */
export function setTablePrivilege(
  state: UserState,
  schema: string,
  privilege: string,
  next: "all" | "none",
): UserState {
  return {
    ...state,
    tablePrivileges: setPrivilege(
      state.tablePrivileges,
      schema,
      privilege,
      next === "all",
      PG_TABLE_PRIVILEGES,
    ),
    partialTablePrivileges: setPrivilege(
      state.partialTablePrivileges,
      schema,
      privilege,
      false,
      PG_TABLE_PRIVILEGES,
    ),
  };
}

/** Comparable form: privileges sorted, empty entries dropped, blank fields normalised. */
function canonical(state: UserState): string {
  const list = (l: readonly ObjectPrivileges[]) =>
    l
      .filter((p) => p.privileges.length > 0)
      .map((p) => ({ object: p.object, privileges: [...p.privileges].sort() }))
      .sort((a, b) => a.object.localeCompare(b.object));
  const attributes = state.attributes
    ? {
        ...state.attributes,
        validUntil: state.attributes.validUntil?.trim() ? state.attributes.validUntil.trim() : null,
      }
    : null;
  return JSON.stringify({
    name: state.name,
    host: state.host,
    globalPrivileges: [...state.globalPrivileges].sort(),
    limits: state.limits,
    attributes,
    databasePrivileges: list(state.databasePrivileges),
    schemaPrivileges: list(state.schemaPrivileges),
    tablePrivileges: list(state.tablePrivileges),
    partialTablePrivileges: list(state.partialTablePrivileges),
  });
}

/** Whether Apply has anything to do. */
export function isDirty(draft: UserDraft): boolean {
  if (draft.original === null) return true;
  return draft.password !== null || canonical(draft.original) !== canonical(draft.state);
}

/** Problems that keep Apply disabled. */
export function draftProblems(draft: UserDraft, driver: Driver): string[] {
  const s = draft.state;
  const problems: string[] = [];
  if (s.name.trim() === "")
    problems.push(driver === "postgres" ? "Enter a role name" : "Enter a user name");
  const limits = s.limits;
  if (limits && Object.values(limits).some((n) => !Number.isInteger(n) || n < 0))
    problems.push("Resource limits are whole numbers, 0 = unlimited");
  const a = s.attributes;
  if (a && (!Number.isInteger(a.connectionLimit) || a.connectionLimit < -1))
    problems.push("Connection limit is a whole number, -1 = no limit");
  return problems;
}

/** What Apply sends. Names are trimmed; a blank host is `%`. */
export function toChange(draft: UserDraft): UserChange {
  const s = draft.state;
  const edited: UserState = {
    ...s,
    name: s.name.trim(),
    host: s.host === null ? null : s.host.trim() || "%",
    attributes: s.attributes
      ? { ...s.attributes, validUntil: s.attributes.validUntil?.trim() || null }
      : null,
  };
  return { original: draft.original, edited, password: draft.password };
}

/** The Drop change of an account. */
export function dropChange(state: UserState): UserChange {
  return { original: state, edited: null, password: null };
}

/** Databases the Database Privileges tab offers: the server's, plus any with grants. */
export function privilegeDatabases(serverDatabases: readonly string[], state: UserState): string[] {
  const names = new Set(serverDatabases);
  for (const p of state.databasePrivileges) names.add(p.object);
  return [...names].sort((a, b) => a.localeCompare(b));
}
