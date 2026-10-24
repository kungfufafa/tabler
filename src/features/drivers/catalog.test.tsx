import { render, screen } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { AssistantPanel } from "@/features/assistant/AssistantPanel";
import { DriverBadge } from "@/features/connections/badges";
import { DRIVER_LABEL, KIND_LABEL, KIND_SHORT, KIND_TINT } from "@/features/connections/colors";
import { sortProfiles } from "@/features/connections/list";
import { openConnectionDialog, useConnectionDialog } from "@/features/connections/store";
import { QueryTab } from "@/features/editor/QueryTab";
import type { GuardRequest } from "@/features/safety/guard";
import { requestWrites } from "@/features/safety/policy";
import { StructureTab } from "@/features/structure/StructureTab";
import { highlightSql } from "@/features/structure/sqlHighlight";
import { ipc, type ConnectionInfo, type ConnectionProfile, type Driver } from "@/lib/ipc";
import { SEED_PROFILES } from "@/lib/ipc/mock/fixtures";
import { ACCENT_PRESETS } from "@/lib/theme";
import { loadAppInfo, useAppInfoStore } from "@/stores/appInfo";
import { useConnectionsStore } from "@/stores/connections";
import {
  CORE_DRIVERS,
  CORE_KINDS,
  coreDialect,
  coreDriver,
  databaseKind,
  DESCRIPTORS,
  isCoreDriver,
  kindDriver,
  kindOf,
  KINDS,
  requireCoreDialect,
  visibleKinds,
} from "./catalog";

beforeAll(async () => {
  // Load the mock backend before the first call (the transport imports it lazily).
  await import("@/lib/ipc/mock");
});

describe("core drivers", () => {
  it("are PostgreSQL, MySQL and SQLite, whose dialects have the same names", () => {
    expect(CORE_DRIVERS).toEqual(["postgres", "mysql", "sqlite"]);
    for (const d of CORE_DRIVERS) {
      expect(isCoreDriver(d)).toBe(true);
      expect(coreDialect(d)).toBe(d);
      expect(requireCoreDialect(d)).toBe(d);
    }
    expect(isCoreDriver("redis")).toBe(false);
    expect(isCoreDriver("libsql")).toBe(false);
  });

  it("have no core dialect for another driver: null while rendering, an error on an action", () => {
    expect(coreDialect("duckdb")).toBeNull();
    expect(coreDialect("sqlserver")).toBeNull();
    expect(() => requireCoreDialect("duckdb", "Copy as UPDATE")).toThrow(
      "Copy as UPDATE is not available for DuckDB connections",
    );
    // The connection form opens core profiles only.
    expect(() => coreDriver("sqlserver")).toThrow("non-core driver sqlserver");
  });
});

describe("the driver catalog", () => {
  it("has every driver and every database of the picker", () => {
    expect(Object.keys(DESCRIPTORS)).toHaveLength(16);
    expect(KINDS).toHaveLength(20);
    // The drivers of every build are the ones no cargo feature builds.
    const core = Object.values(DESCRIPTORS).filter((d) => d.cargoFeature === null);
    expect(core.map((d) => d.driver)).toEqual(CORE_DRIVERS);
    expect(CORE_KINDS).toEqual(["postgres", "mysql", "mariadb", "sqlite"]);
  });

  it("gives every driver a database of its own id", () => {
    for (const { driver } of Object.values(DESCRIPTORS)) {
      expect(databaseKind(driver)).toBe(driver);
      expect(kindDriver(databaseKind(driver))).toBe(driver);
    }
  });

  it("maps the flavors a driver serves to their own databases", () => {
    expect(databaseKind("mysql", "mariadb")).toBe("mariadb");
    expect(databaseKind("mysql", "default")).toBe("mysql");
    expect(databaseKind("postgres", null, "cockroachdb")).toBe("cockroachdb");
    expect(databaseKind("postgres", null, "redshift")).toBe("redshift");
    // A PostgreSQL flavor on another driver means nothing.
    expect(databaseKind("mysql", null, "redshift")).toBe("mysql");
    expect(kindDriver("mariadb")).toBe("mysql");
    expect(kindDriver("greenplum")).toBe("postgres");
    const pg = SEED_PROFILES.find((p) => p.driver === "postgres")!;
    expect(kindOf(pg)).toBe("postgres");
    expect(kindOf({ ...pg, options: { ...pg.options, pgFlavor: "greenplum" } })).toBe("greenplum");
  });

  it("keeps the core databases' labels, badges and tints", () => {
    expect(CORE_KINDS.map((k) => [KIND_LABEL[k], KIND_SHORT[k], KIND_TINT[k]])).toEqual([
      ["PostgreSQL", "PG", "blue"],
      ["MySQL", "MY", "orange"],
      ["MariaDB", "MA", "purple"],
      ["SQLite", "SQ", "teal"],
    ]);
    expect(CORE_DRIVERS.map((d) => DRIVER_LABEL[d])).toEqual([
      "PostgreSQL",
      "MySQL / MariaDB",
      "SQLite",
    ]);
    expect(DRIVER_LABEL.redis).toBe("Redis");
    // Every tile's colour is an accent preset.
    for (const k of KINDS) expect(ACCENT_PRESETS).toContain(k.tint);
  });
});

describe("the new-connection picker's databases", () => {
  beforeEach(() => useAppInfoStore.setState({ info: null }));

  it("are the core ones until the backend has said otherwise", () => {
    expect(visibleKinds(undefined)).toEqual(["postgres", "mysql", "mariadb", "sqlite"]);
    expect(visibleKinds(null)).toEqual(CORE_KINDS);
    expect(visibleKinds({ kinds: null })).toEqual(CORE_KINDS);
  });

  it("are the 4 core tiles in the mock, a build of the core drivers", async () => {
    const coreAppInfo = await ipc.appInfo();
    expect(coreAppInfo.drivers).toEqual(["postgres", "mysql", "sqlite"]);
    expect(visibleKinds(coreAppInfo)).toEqual(["postgres", "mysql", "mariadb", "sqlite"]);
    await loadAppInfo();
    expect(useAppInfoStore.getState().info).toEqual(coreAppInfo);
  });

  it("follow a fuller build's list, in the picker's order", () => {
    expect(visibleKinds({ kinds: ["redis", "mariadb", "postgres"] })).toEqual([
      "postgres",
      "mariadb",
      "redis",
    ]);
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

  it("is refused by the mock, which has the core drivers only", async () => {
    await expect(
      ipc.profiles.test({
        profile: redis,
        password: null,
        sshPassword: null,
        sshPassphrase: null,
        clearPassword: false,
      }),
    ).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: Redis is not built into this Tabler",
    });
  });
});

function info(driver: Driver): ConnectionInfo {
  return {
    connectionId: "c-other",
    profileId: "p-other",
    name: "Other",
    color: null,
    driver,
    serverVersion: "1.0",
    currentDatabase: null,
    currentUser: null,
    tunnel: null,
    readOnly: false,
  };
}

describe("SQL-only code on a connection of another driver", () => {
  beforeEach(() => {
    useConnectionsStore.setState({ byId: {}, order: [], activeId: null });
    useConnectionsStore.getState().add(info("duckdb"));
    useConnectionsStore.getState().setActive("c-other");
  });

  it("does not hand its SQL to the core classifier or highlighter", () => {
    const query: GuardRequest = { kind: "query", statements: ["SELECT 1"] };
    expect(requestWrites(query, "postgres")).toBe(false);
    // Every statement counts as a write (Safe Mode asks; a read-only connection refuses).
    expect(requestWrites(query, null)).toBe(true);
    expect(highlightSql(null, "SELECT 1\nFROM t")).toEqual([
      [{ text: "SELECT 1", cls: "" }],
      [{ text: "FROM t", cls: "" }],
    ]);
  });

  it("renders the editor, structure and assistant panes as not available", () => {
    render(<QueryTab tab={{ id: "t1", kind: "query", connectionId: "c-other", title: "Query" }} />);
    expect(
      screen.getByText("The SQL editor is not available for DuckDB connections"),
    ).toBeInTheDocument();
    render(
      <StructureTab
        tab={{
          id: "t2",
          kind: "structure",
          connectionId: "c-other",
          table: { schema: "main", name: "t" },
          title: "t",
        }}
      />,
    );
    expect(
      screen.getByText("The structure editor is not available for DuckDB connections"),
    ).toBeInTheDocument();
    render(<AssistantPanel />);
    expect(
      screen.getByText("The assistant is not available for DuckDB connections"),
    ).toBeInTheDocument();
  });
});
