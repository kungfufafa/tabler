/** React Query keys of the admin surfaces. */
export const adminKeys = {
  databaseOptions: (c: string) => ["adminDatabaseOptions", c] as const,
  processes: (c: string) => ["adminProcesses", c] as const,
  /** Every user list of the connection (with and without NOLOGIN roles). */
  users: (c: string) => ["adminUsers", c] as const,
  userList: (c: string, includeNologin: boolean) => ["adminUsers", c, includeNologin] as const,
  /** Every loaded account of the connection. */
  userDetails: (c: string) => ["adminUser", c] as const,
  userDetail: (c: string, key: string) => ["adminUser", c, key] as const,
};

/** Auto-refresh period of Running Queries. */
export const PROCESS_REFRESH_MS = 2000;
