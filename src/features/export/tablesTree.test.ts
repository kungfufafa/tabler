import { describe, expect, it } from "vitest";
import type { TableSummary } from "@/lib/ipc";
import {
  databaseFileBase,
  databaseState,
  groupChosen,
  hasSeveralSchemas,
  matchTables,
  tableKey,
  tableLabel,
  treeTables,
  uniqueDatabaseBases,
} from "./tablesTree";

const summary = (schema: string | null, name: string): TableSummary => ({
  name,
  schema,
  kind: "table",
  rowEstimate: null,
  sizeBytes: null,
  comment: null,
});

describe("multi-database table tree", () => {
  const dev = treeTables("tabler_dev", [
    summary("public", "users"),
    summary("public", "posts"),
    summary("inventory", "items"),
  ]);
  const shop = treeTables("shop", [summary("public", "orders")]);

  it("keys tables by database, schema and name", () => {
    expect(tableKey("a", { schema: "s", name: "t" })).not.toBe(
      tableKey("b", { schema: "s", name: "t" }),
    );
    expect(tableKey(null, { schema: null, name: "t" })).toBe(
      tableKey(null, { schema: null, name: "t" }),
    );
    expect(dev[0]!.table).toEqual({ schema: "public", name: "users" });
  });

  it("labels with the schema only when the database has several", () => {
    expect(hasSeveralSchemas(dev)).toBe(true);
    expect(hasSeveralSchemas(shop)).toBe(false);
    expect(tableLabel(dev[2]!, true)).toBe("inventory.items");
    expect(tableLabel(shop[0]!, false)).toBe("orders");
  });

  it("matches names, schema.name and the database", () => {
    expect(matchTables(dev, "").length).toBe(3);
    expect(matchTables(dev, "POST").map((t) => t.table.name)).toEqual(["posts"]);
    expect(matchTables(dev, "inventory.").map((t) => t.table.name)).toEqual(["items"]);
    expect(matchTables(dev, "tabler").length).toBe(3);
  });

  it("groups the checked tables per database, the current one first", () => {
    const loaded = new Map([
      ["shop", shop],
      ["tabler_dev", dev],
    ]);
    const checked = new Set([dev[1]!.key, shop[0]!.key, dev[0]!.key]);
    expect(groupChosen(["shop", "tabler_dev", "empty"], loaded, checked, "tabler_dev")).toEqual([
      {
        database: "tabler_dev",
        tables: [
          { schema: "public", name: "users" },
          { schema: "public", name: "posts" },
        ],
      },
      { database: "shop", tables: [{ schema: "public", name: "orders" }] },
    ]);
    expect(groupChosen(["shop"], loaded, new Set(), "shop")).toEqual([]);
  });

  it("gives a database checkbox its tri-state", () => {
    expect(databaseState(undefined, new Set(), false)).toBe(false);
    expect(databaseState(undefined, new Set(), true)).toBe(true);
    expect(databaseState(dev, new Set([dev[0]!.key]), false)).toBe("indeterminate");
    expect(databaseState(dev, new Set(dev.map((t) => t.key)), false)).toBe(true);
    expect(databaseState([], new Set(), false)).toBe(false);
  });

  it("names a database's file or folder safely", () => {
    expect(databaseFileBase("tabler_dev")).toBe("tabler_dev");
    expect(databaseFileBase("my db/../x")).toBe("my_db_.._x");
    expect(databaseFileBase("..")).toBe("database");
  });

  describe("uniqueDatabaseBases", () => {
    it("separates names that reduce to the same base", () => {
      expect(uniqueDatabaseBases(["my db", "my_db"])).toEqual(["my_db", "my_db_2"]);
      expect(uniqueDatabaseBases(["$$", "@@"])).toEqual(["database", "database_2"]);
      expect(uniqueDatabaseBases(["a b", "a_b", "a-b", "a b"])).toEqual([
        "a_b",
        "a_b_2",
        "a-b",
        "a_b_3",
      ]);
    });

    it("compares case-insensitively (one file on a case-insensitive disk)", () => {
      expect(uniqueDatabaseBases(["Sales", "sales"])).toEqual(["Sales", "sales_2"]);
      expect(uniqueDatabaseBases(["SALES", "Sales", "sales"])).toEqual([
        "SALES",
        "Sales_2",
        "sales_3",
      ]);
    });

    it("never hands out a name another database keeps", () => {
      // The second `a` would be `a_2`, which the third database already is.
      expect(uniqueDatabaseBases(["a", "a_2", "a"])).toEqual(["a", "a_2", "a_3"]);
      expect(uniqueDatabaseBases(["a", "a", "a_2"])).toEqual(["a", "a_3", "a_2"]);
      expect(uniqueDatabaseBases(["a", "A_2", "A", "a"])).toEqual(["a", "A_2", "A_3", "a_4"]);
      for (const names of [
        ["x", "X", "x_2", "X_2", "x"],
        ["db", "db", "db", "db_2", "db_3"],
        ["é", "É", "$", "@", "#"],
      ]) {
        const out = uniqueDatabaseBases(names).map((n) => n.toLowerCase());
        expect(new Set(out).size, names.join()).toBe(names.length);
      }
    });

    it("leaves distinct names as they are", () => {
      expect(uniqueDatabaseBases([])).toEqual([]);
      expect(uniqueDatabaseBases(["tabler_dev"])).toEqual(["tabler_dev"]);
      expect(uniqueDatabaseBases(["shop", "blog", "tabler_dev"])).toEqual([
        "shop",
        "blog",
        "tabler_dev",
      ]);
    });
  });
});
