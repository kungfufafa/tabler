import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DriverBadge } from "@/features/connections/badges";
import { sortProfiles } from "@/features/connections/list";
import { openConnectionDialog, useConnectionDialog } from "@/features/connections/store";
import type { ConnectionProfile } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { CORE_DRIVERS, coreDialect, coreDriver, isCoreDriver } from "./catalog";

describe("core drivers", () => {
  it("are PostgreSQL, MySQL and SQLite, whose dialects have the same names", () => {
    expect(CORE_DRIVERS).toEqual(["postgres", "mysql", "sqlite"]);
    for (const d of CORE_DRIVERS) {
      expect(isCoreDriver(d)).toBe(true);
      expect(coreDialect(d)).toBe(d);
    }
    expect(isCoreDriver("redis")).toBe(false);
    expect(isCoreDriver("libsql")).toBe(false);
  });

  it("a SQL-only path refuses another driver rather than treat it as PostgreSQL", () => {
    expect(() => coreDriver("sqlserver")).toThrow("non-core driver sqlserver");
    expect(() => coreDialect("duckdb")).toThrow("non-core driver duckdb");
  });
});

/** A connection a newer Tabler saved with a driver this one has no form for. */
const redis: ConnectionProfile = {
  ...SEED_PROFILES[0]!,
  id: "p-redis",
  name: "Cache",
  driver: "redis",
};

describe("a profile of another driver in the launcher", () => {
  it("gets a badge and a place in the driver sort", () => {
    const { container } = render(<DriverBadge driver="redis" color="red" />);
    expect(container).toHaveTextContent("RE");
    const sorted = sortProfiles([redis, SEED_PROFILES[0]!], "driver").map((p) => p.driver);
    expect(sorted).toEqual(["postgres", "redis"]);
  });

  it("does not open the core form", () => {
    useConnectionDialog.setState({ open: false, profile: null, prefill: null });
    openConnectionDialog(redis);
    expect(useConnectionDialog.getState()).toMatchObject({ open: false, profile: null });
    openConnectionDialog(SEED_PROFILES[0]!);
    expect(useConnectionDialog.getState()).toMatchObject({ open: true });
  });
});
