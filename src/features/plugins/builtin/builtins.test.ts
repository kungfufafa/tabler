import { describe, expect, it, vi } from "vitest";
import type { GridColumn } from "@/features/datagrid/types";
import type { ColumnMeta, Value } from "@/lib/ipc";
import type { HostServices, PluginInvocation } from "../types";
import { updateStatements } from "./copyAsUpdate";
import { dummyInserts, dummyText, dummyValue, seededRng, skipsColumn } from "./dummyData";
import { urlFromValue } from "./openUrl";
import { SQL_FORMATTER } from "./sqlFormatter";
import { uuidTarget, uuidV4, UUID_GENERATOR } from "./uuid";

const { formatQuery } = vi.hoisted(() => ({ formatQuery: vi.fn(async () => {}) }));
vi.mock("@/features/editor/controller", () => ({ formatQuery }));

const col = (name: string, over: Partial<GridColumn> = {}): GridColumn => ({
  id: name,
  name,
  kind: "text",
  dataType: "text",
  nullable: true,
  isPk: false,
  hasDefault: false,
  isAutoInc: false,
  editable: true,
  align: "left",
  ...over,
});

const meta = (name: string, over: Partial<ColumnMeta> = {}): ColumnMeta => ({
  name,
  ordinal: 1,
  dataType: "text",
  kind: "text",
  nullable: true,
  defaultExpr: null,
  isPk: false,
  pkOrdinal: null,
  isAutoIncrement: false,
  isGenerated: false,
  isUnsigned: false,
  comment: null,
  enumValues: null,
  charMaxLen: null,
  numericPrecision: null,
  numericScale: null,
  collation: null,
  arrayElemType: null,
  extra: null,
  ...over,
});

function host(): HostServices & { calls: unknown[][] } {
  const calls: unknown[][] = [];
  const rec =
    (name: string) =>
    async (...args: unknown[]) => {
      calls.push([name, ...args]);
    };
  return {
    calls,
    execute: vi.fn(),
    alert: rec("alert"),
    insertToEditor: rec("insertToEditor"),
    replaceSelection: rec("replaceSelection"),
    insertToClipboard: rec("insertToClipboard"),
    openURL: rec("openURL"),
    notify: (title, message) => void calls.push(["notify", title, message]),
    loadFile: vi.fn(),
  };
}

function invocation(over: Partial<PluginInvocation> = {}): PluginInvocation {
  return {
    source: "global",
    connectionId: "c1",
    driver: "postgres",
    item: null,
    clickedRow: null,
    clickedColumn: null,
    selectedRows: [],
    editorTabId: null,
    sql: null,
    ...over,
  };
}

describe("Open URL", () => {
  it("finds the web link a cell holds", () => {
    const text = (v: string): Value => ({ t: "text", v });
    expect(urlFromValue(text("https://tableplus.com/docs"))).toBe("https://tableplus.com/docs");
    expect(urlFromValue(text("  www.example.com  "))).toBe("https://www.example.com/");
    expect(urlFromValue(text("see http://a.io/x?y=1, then"))).toBe("http://a.io/x?y=1");
    expect(urlFromValue(text("mailto:me@x.io"))).toBeNull();
    expect(urlFromValue(text("file:///etc/passwd"))).toBeNull();
    expect(urlFromValue(text("javascript:alert(1)"))).toBeNull();
    expect(urlFromValue({ t: "null" })).toBeNull();
    expect(urlFromValue({ t: "int", v: 3 })).toBeNull();
  });
});

describe("Copy as UPDATE Statement", () => {
  const columns = [col("id", { isPk: true, kind: "int" }), col("name"), col("note")];
  it("matches rows on the primary key", () => {
    const rows = [
      {
        columns,
        cells: [{ t: "int", v: 1 }, { t: "text", v: "O'Hara" }, { t: "null" }] as Value[],
      },
      {
        columns,
        cells: [
          { t: "int", v: 2 },
          { t: "text", v: "b" },
          { t: "text", v: "x" },
        ] as Value[],
      },
    ];
    expect(updateStatements("postgres", { schema: "public", name: "users" }, rows)).toEqual([
      `UPDATE "public"."users" SET "name" = 'O''Hara', "note" = NULL WHERE "id" = 1;`,
      `UPDATE "public"."users" SET "name" = 'b', "note" = 'x' WHERE "id" = 2;`,
    ]);
    expect(updateStatements("mysql", { schema: null, name: "users" }, rows.slice(1))).toEqual([
      "UPDATE `users` SET `name` = 'b', `note` = 'x' WHERE `id` = 2;",
    ]);
  });

  it("uses every column without a key, and skips unsaved rows", () => {
    const plain = [col("a"), col("b")];
    expect(
      updateStatements("sqlite", { schema: null, name: "t" }, [
        { columns: plain, cells: [{ t: "text", v: "x" }, { t: "null" }] },
      ]),
    ).toEqual([`UPDATE "t" SET "a" = 'x', "b" = NULL WHERE "a" = 'x' AND "b" IS NULL;`]);
    expect(
      updateStatements("postgres", { schema: null, name: "users" }, [
        { columns, cells: [{ t: "default" }, { t: "text", v: "new" }, { t: "null" }] },
      ]),
    ).toEqual([]);
  });
});

describe("UUID Generator", () => {
  it("formats a version 4 UUID", () => {
    const uuid = uuidV4(new Uint8Array(16).fill(0xff));
    expect(uuid).toBe("ffffffff-ffff-4fff-bfff-ffffffffffff");
    expect(uuidV4()).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(() => uuidV4(new Uint8Array(3))).toThrow();
  });

  it("goes to the editor, the clicked cell or the clipboard", async () => {
    expect(uuidTarget({ source: "editor", editorTabId: "q1" })).toBe("editor");
    expect(uuidTarget({ source: "grid", editorTabId: "q1", stageClickedCell: () => {} })).toBe(
      "cell",
    );
    expect(uuidTarget({ source: "grid", editorTabId: null })).toBe("clipboard");
    expect(uuidTarget({ source: "global", editorTabId: null })).toBe("clipboard");

    const stage = vi.fn();
    const uuidCol = col("id", { kind: "uuid" });
    await UUID_GENERATOR.commands[0]!.run(
      invocation({
        source: "grid",
        clickedRow: { columns: [uuidCol], cells: [{ t: "null" }] },
        clickedColumn: 0,
        stageClickedCell: stage,
      }),
      host(),
    );
    expect(stage).toHaveBeenCalledWith({ t: "uuid", v: expect.stringMatching(/^[0-9a-f-]{36}$/) });
    const h = host();
    await UUID_GENERATOR.commands[0]!.run(invocation({ source: "editor", editorTabId: "q1" }), h);
    expect(h.calls[0]?.[0]).toBe("insertToEditor");
  });
});

describe("SQL Formatter", () => {
  it("formats the active query tab's editor, else says where it works", async () => {
    const run = SQL_FORMATTER.commands[0]!.run;
    await run(invocation({ source: "editor", editorTabId: "q1" }), host());
    expect(formatQuery).toHaveBeenCalledWith("q1");
    const h = host();
    await run(invocation(), h);
    expect(h.calls).toEqual([["notify", "SQL Format", expect.stringContaining("SQL query tab")]]);
  });
});

describe("Dummy Data", () => {
  it("fills columns by kind and name", () => {
    const rng = seededRng(7);
    expect(dummyText("email", 0, rng, null)).toMatch(/^[a-z-]+\.[a-z-]+1@example\.com$/);
    expect(dummyText("description", 0, rng, 8)).toHaveLength(8);
    expect(dummyValue(meta("active", { kind: "bool" }), 0, rng)?.t).toBe("bool");
    expect(dummyValue(meta("id", { kind: "uuid" }), 0, rng)).toMatchObject({ t: "uuid" });
    expect(dummyValue(meta("born", { kind: "date" }), 0, rng)).toMatchObject({
      t: "date",
      v: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
    });
    expect(
      dummyValue(meta("price", { kind: "decimal", numericPrecision: 6, numericScale: 2 }), 0, rng),
    ).toMatchObject({ t: "decimal", v: expect.stringMatching(/^\d{1,4}\.\d{2}$/) });
    const mood = dummyValue(meta("mood", { kind: "enum", enumValues: ["ok", "sad"] }), 0, rng);
    expect(["ok", "sad"]).toContain(mood?.t === "enum" ? mood.v : null);
    expect(dummyValue(meta("blob", { kind: "bytes" }), 0, rng)).toBeNull();
    // A foreign key: NULL when it may be, else a small id.
    expect(dummyValue(meta("owner_id", { kind: "int" }), 0, rng, true)).toEqual({ t: "null" });
    expect(skipsColumn(meta("id", { isAutoIncrement: true }))).toBe(true);
    expect(skipsColumn(meta("id", { isPk: true, defaultExpr: "gen_random_uuid()" }))).toBe(true);
  });

  it("builds reproducible INSERTs, 100 rows per statement", () => {
    const table = {
      table: { schema: "public", name: "people" },
      columns: [
        meta("id", { kind: "int", isPk: true, isAutoIncrement: true }),
        meta("email"),
        meta("age", { kind: "int", dataType: "integer" }),
        meta("photo", { kind: "bytes" }),
      ],
      foreignKeys: [],
    };
    const a = dummyInserts("postgres", table, 250, seededRng(1));
    const b = dummyInserts("postgres", table, 250, seededRng(1));
    expect(a).toEqual(b);
    expect(a).toHaveLength(3);
    expect(a[0]).toMatch(/^INSERT INTO "public"\."people" \("email", "age"\) VALUES\n {2}\('/);
    expect(a[2]!.split("\n")).toHaveLength(51);
    expect(dummyInserts("mysql", table, 1, seededRng(1))[0]).toMatch(
      /^INSERT INTO `public`\.`people` \(`email`, `age`\) VALUES/,
    );
    // Nothing Tabler can fill: the defaults.
    const bare = { ...table, columns: [table.columns[0]!, table.columns[3]!] };
    expect(dummyInserts("mysql", bare, 2, seededRng(1))).toEqual([
      "INSERT INTO `public`.`people` () VALUES ();",
      "INSERT INTO `public`.`people` () VALUES ();",
    ]);
  });
});
