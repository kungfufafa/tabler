import { afterEach, describe, expect, it } from "vitest";
import { ipc } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";
import { resetMockObjects } from "@/lib/ipc/mock/objects";

async function connect(profileId: string): Promise<string> {
  return (await ipc.connections.connect(profileId)).connectionId;
}

afterEach(() => {
  resetMockDdl();
  resetMockObjects();
});

describe("mock sidebar objects", () => {
  it("lists every schema's items for Search in Database…", async () => {
    const c = await connect("p-local-pg");
    const all = await ipc.objects.databaseItems(c);
    const schemas = new Set(all.tables.map((t) => t.schema));
    expect(schemas).toEqual(new Set(["public", "inventory"]));
    expect(all.routines.map((r) => r.name)).toContain("slugify");
    const withSystem = await ipc.objects.databaseItems(c, true);
    expect(withSystem.tables.length).toBeGreaterThanOrEqual(all.tables.length);
  });

  it("lists PG routines and sequences, and opens their definitions", async () => {
    const c = await connect("p-local-pg");
    const routines = await ipc.objects.routines(c, "public");
    expect(routines.map((r) => `${r.name}(${r.identityArguments})`)).toContain(
      "add(a integer, b integer)",
    );
    const slugify = routines.find((r) => r.name === "slugify")!;
    const def = await ipc.objects.routineDefinition(c, {
      schema: "public",
      name: "slugify",
      kind: slugify.kind,
      identityArguments: slugify.identityArguments,
    });
    expect(def).toMatch(/^CREATE OR REPLACE FUNCTION public\.slugify\(value text\)/);
    const seqs = await ipc.objects.sequences(c, "public");
    expect(seqs.map((s) => s.name)).toContain("users_id_seq");
    expect(
      await ipc.objects.sequenceDefinition(c, { schema: "public", name: "users_id_seq" }),
    ).toMatch(/OWNED BY "public"\."users"\."id"/);
    expect((await ipc.objects.systemSchemas(c)).map((s) => s.name)).toEqual([
      "information_schema",
      "pg_catalog",
      "pg_toast",
    ]);
  });

  it("drops tables through the DDL mock and routines in its own catalog", async () => {
    const c = await connect("p-local-pg");
    const op = {
      op: "dropTables" as const,
      tables: [
        { table: { schema: "public", name: "tags" }, kind: "table" as const },
        { table: { schema: "public", name: "active_users" }, kind: "view" as const },
      ],
      cascade: true,
      ignoreForeignKeys: false,
    };
    // One statement per kind, views first; later CASCADE statements tolerate objects the
    // earlier cascade already removed (mirrors sqlgen/objects.rs).
    expect((await ipc.objects.preview(c, op)).statements).toEqual([
      'DROP VIEW "public"."active_users" CASCADE',
      'DROP TABLE IF EXISTS "public"."tags" CASCADE',
    ]);
    const r = await ipc.objects.apply(c, op);
    expect(r.error).toBeNull();
    const names = (await ipc.schema.tables(c, "public")).map((t) => t.name);
    expect(names).not.toContain("tags");
    expect(names).not.toContain("active_users");

    await ipc.objects.apply(c, {
      op: "dropRoutines",
      routines: [
        { schema: "public", name: "slugify", kind: "function", identityArguments: "value text" },
      ],
      cascade: false,
    });
    expect((await ipc.objects.routines(c, "public")).map((x) => x.name)).not.toContain("slugify");
  });

  it("clones a table and keeps comments set in the overview", async () => {
    const c = await connect("p-shop-mysql");
    const source = { schema: "tabler_dev", name: "users" };
    const plan = await ipc.objects.preview(c, {
      op: "cloneTable",
      source,
      newName: "users_copy",
      withData: true,
    });
    expect(plan.statements[0]).toBe(
      "CREATE TABLE `tabler_dev`.`users_copy` LIKE `tabler_dev`.`users`",
    );
    expect(plan.statements[1]).toMatch(/^INSERT INTO `tabler_dev`\.`users_copy` \(`id`, /);
    expect(
      (
        await ipc.objects.apply(c, {
          op: "cloneTable",
          source,
          newName: "users_copy",
          withData: true,
        })
      ).error,
    ).toBeNull();
    expect((await ipc.schema.tables(c, "tabler_dev")).map((t) => t.name)).toContain("users_copy");

    await ipc.objects.apply(c, {
      op: "setComment",
      target: { table: source, kind: "table" },
      comment: "People",
    });
    const users = (await ipc.objects.overview(c, "tabler_dev")).find((i) => i.name === "users");
    expect(users).toMatchObject({ comment: "People", engine: "InnoDB", charset: "utf8mb4" });
    // MySQL has no routines in the SQLite fixture, nor sequences.
    expect(await ipc.objects.sequences(c, "tabler_dev")).toEqual([]);
    const lite = await connect("p-sample-sqlite");
    expect(await ipc.objects.routines(lite, "main")).toEqual([]);
  });
});
