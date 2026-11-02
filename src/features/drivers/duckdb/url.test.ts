import { describe, expect, it } from "vitest";
import {
  DEFAULT_FORM,
  formToProfileInput,
  type ConnectionFormValues,
} from "@/features/connections/schema";
import lane from ".";
import { formatDuckdbUrl, parseDuckdbUrl } from "./url";

// DuckDB URLs: TablePlus's `duckdb://<path>` and SQLAlchemy's `duckdb:///:memory:`, DuckDB's own
// keys read, a key kept out of the fields, and every profile written as a URL that reads back to
// the same profile.

const ui = lane.kinds!.duckdb!;

const base = (values: Partial<ConnectionFormValues> = {}): ConnectionFormValues => ({
  ...DEFAULT_FORM,
  ...ui.defaults!(),
  name: "n",
  driver: "duckdb",
  ...values,
});

const saved = (values: ConnectionFormValues) =>
  formToProfileInput(values, null, "id-1", ui).profile;

/** The profile a URL makes, through the form, as "New connection from URL…" saves it. */
function profileOf(url: string) {
  const parsed = parseDuckdbUrl(url);
  if (!parsed) throw new Error(`not read: ${url}`);
  return saved(base({ ...parsed.values }));
}

describe("DuckDB URLs", () => {
  it("read the file, its name and DuckDB's keys", () => {
    expect(parseDuckdbUrl("duckdb:///Users/me/data/sales.duckdb")).toEqual({
      values: {
        sqlitePath: "/Users/me/data/sales.duckdb",
        readOnly: false,
        driverParams: { autoloadExtensions: false },
        name: "sales.duckdb",
      },
      password: null,
    });
    expect(
      parseDuckdbUrl(
        "DuckDB:///C:/data/x.duckdb?access_mode=READ_ONLY&autoload_known_extensions=true&encryption_key=s%40cret",
      ),
    ).toEqual({
      values: {
        sqlitePath: "C:/data/x.duckdb",
        readOnly: true,
        driverParams: { autoloadExtensions: true },
        name: "x.duckdb",
      },
      password: "s@cret",
    });
    expect(parseDuckdbUrl("duckdb://C:/data/x.duckdb?read_only=1")?.values).toMatchObject({
      sqlitePath: "C:/data/x.duckdb",
      readOnly: true,
    });
    expect(parseDuckdbUrl("duckdb://~/x.ddb?autoload=yes")?.values).toMatchObject({
      sqlitePath: "~/x.ddb",
      driverParams: { autoloadExtensions: true },
    });
    expect(parseDuckdbUrl("duckdb:/abs/x.duckdb")?.values.sqlitePath).toBe("/abs/x.duckdb");
    expect(parseDuckdbUrl("duckdb:///%5C%5Cserver%5Cshare%5Cx.duckdb")?.values.sqlitePath).toBe(
      "\\\\server\\share\\x.duckdb",
    );
  });

  it("read an in-memory database without a name of its own", () => {
    for (const url of ["duckdb://", "duckdb:///", "duckdb:///:memory:", "duckdb://:memory:"]) {
      const read = parseDuckdbUrl(url);
      expect(read?.values.sqlitePath, url).toBe(":memory:");
      expect(read?.values.name, url).toBeUndefined();
    }
  });

  it("are no URL for another scheme, MotherDuck, a relative path or an unknown access mode", () => {
    for (const url of [
      "sqlite:///x.db",
      "duckdb:md:my_db",
      "duckdb://data/x.duckdb",
      "duckdb:///x.duckdb?access_mode=readonly",
      "motherduck://x",
    ])
      expect(parseDuckdbUrl(url), url).toBeNull();
  });

  it("write every profile as a URL that reads back to it, never with the key", () => {
    for (const values of [
      { sqlitePath: "/Users/me/data/sales.duckdb" },
      { sqlitePath: "/Users/me/my data/a#b?c%d.duckdb", readOnly: true },
      { sqlitePath: "C:\\Users\\me\\x.duckdb", driverParams: { autoloadExtensions: true } },
      { sqlitePath: "C:/data/ünï.duckdb", readOnly: true },
      { sqlitePath: "\\\\server\\share\\x.duckdb" },
      { sqlitePath: "~/x.ddb" },
      { sqlitePath: ":memory:", driverParams: { autoloadExtensions: true } },
    ] satisfies Partial<ConnectionFormValues>[]) {
      const p = saved(base({ ...values, password: "secret-key" }));
      const url = formatDuckdbUrl(p)!;
      expect(url, JSON.stringify(values)).toMatch(/^duckdb:\/\//);
      expect(url).not.toContain("secret");
      const back = profileOf(url);
      expect(back.sqlitePath, url).toBe(p.sqlitePath);
      expect(back.options.readOnly, url).toBe(p.options.readOnly);
      expect(back.options.driverParams, url).toEqual(p.options.driverParams);
    }
    expect(formatDuckdbUrl(saved(base({ sqlitePath: "/x.duckdb" })))).toBe(
      "duckdb:///x.duckdb?name=n&statusColor=2279DC&env=local",
    );
    expect(formatDuckdbUrl(saved(base({ sqlitePath: ":memory:", readOnly: false })))).toBe(
      "duckdb:///:memory:?name=n&statusColor=2279DC&env=local",
    );
    expect(formatDuckdbUrl(saved(base({ sqlitePath: "C:/d/x.duckdb", readOnly: true })))).toBe(
      "duckdb:///C:/d/x.duckdb?access_mode=read_only&name=n&statusColor=2279DC&env=local",
    );
    // The query every URL carries (`profileUrlQuery`), after DuckDB's own keys.
    const tagged = saved(base({ sqlitePath: "/x.duckdb", name: "Sales" }));
    tagged.options = { ...tagged.options, tag: "prod", safeMode: "safe" };
    expect(formatDuckdbUrl(tagged)).toBe(
      "duckdb:///x.duckdb?name=Sales&statusColor=2279DC&env=prod&safeModeLevel=2",
    );
  });
});
