import { afterEach, describe, expect, it } from "vitest";
import { ipc, type DdlOp, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";

async function connect(profileId: string): Promise<string> {
  return (await ipc.connections.connect(profileId)).connectionId;
}

const PG_USERS: TableRef = { schema: "public", name: "users" };

afterEach(() => resetMockDdl());

describe("mock preview_ddl", () => {
  it("PostgreSQL: one transactional ALTER per op", async () => {
    const c = await connect("p-local-pg");
    const ops: DdlOp[] = [
      { op: "renameColumn", table: PG_USERS, name: "nickname", newName: "nick" },
      { op: "alterColumnType", table: PG_USERS, name: "nick", newType: "varchar(40)", using: null },
      { op: "alterColumnDefault", table: PG_USERS, name: "score", defaultExpr: null },
      { op: "setColumnComment", table: PG_USERS, name: "bio", comment: "it's me" },
    ];
    const plan = await ipc.ddl.preview(c, PG_USERS, ops);
    expect(plan).toEqual({
      transactional: true,
      warnings: [],
      statements: [
        'ALTER TABLE "public"."users" RENAME COLUMN "nickname" TO "nick"',
        'ALTER TABLE "public"."users" ALTER COLUMN "nick" TYPE varchar(40)',
        'ALTER TABLE "public"."users" ALTER COLUMN "score" DROP DEFAULT',
        `COMMENT ON COLUMN "public"."users"."bio" IS 'it''s me'`,
      ],
    });
  });

  it("MySQL: MODIFY COLUMN with the full definition, not transactional, AFTER honoured", async () => {
    const c = await connect("p-shop-mysql");
    const t = { schema: "tabler_dev", name: "users" };
    const plan = await ipc.ddl.preview(c, t, [
      { op: "alterColumnNullable", table: t, name: "email", nullable: true },
      {
        op: "addColumn",
        table: t,
        column: {
          name: "slug",
          dataType: "varchar(80)",
          nullable: false,
          defaultExpr: "''",
          autoIncrement: false,
          comment: "url part",
          collation: null,
          unsigned: false,
          onUpdate: null,
        },
        after: "email",
      },
    ]);
    expect(plan.transactional).toBe(false);
    expect(plan.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` MODIFY COLUMN `email` text NULL",
      "ALTER TABLE `tabler_dev`.`users` ADD COLUMN `slug` varchar(80) NOT NULL DEFAULT '' COMMENT 'url part' AFTER `email`",
    ]);
  });

  it("SQLite: a type change becomes a table rebuild with a warning", async () => {
    const c = await connect("p-sample-sqlite");
    const t = { schema: "main", name: "tags" };
    const plan = await ipc.ddl.preview(c, t, [
      { op: "renameColumn", table: t, name: "name", newName: "label" },
      { op: "alterColumnType", table: t, name: "label", newType: "VARCHAR(40)", using: null },
    ]);
    expect(plan.transactional).toBe(false);
    expect(plan.warnings[0]).toMatch(/rebuilt from introspected metadata/);
    expect(plan.statements[0]).toBe("PRAGMA foreign_keys = OFF");
    expect(plan.statements[1]).toBe("BEGIN");
    expect(plan.statements[2]).toContain('CREATE TABLE "main"."tags__tabler_rebuild"');
    expect(plan.statements[2]).toContain('"label" VARCHAR(40)');
    expect(plan.statements).toContain(
      'INSERT INTO "main"."tags__tabler_rebuild" ("id", "label") SELECT "id", "name" FROM "main"."tags"',
    );
    expect(plan.statements.at(-1)).toBe("PRAGMA foreign_keys = ON");
  });

  it("SQLite: simple renames stay plain ALTER TABLE statements; comments warn", async () => {
    const c = await connect("p-sample-sqlite");
    const t = { schema: "main", name: "tags" };
    const plan = await ipc.ddl.preview(c, t, [
      { op: "renameColumn", table: t, name: "name", newName: "label" },
      { op: "setColumnComment", table: t, name: "label", comment: "x" },
    ]);
    expect(plan).toEqual({
      transactional: true,
      warnings: ["SQLite does not store table or column comments; ignored"],
      statements: ['ALTER TABLE "main"."tags" RENAME COLUMN "name" TO "label"'],
    });
  });

  it("rejects unknown columns like the planner", async () => {
    const c = await connect("p-local-pg");
    await expect(
      ipc.ddl.preview(c, PG_USERS, [{ op: "dropColumn", table: PG_USERS, name: "nope" }]),
    ).rejects.toMatchObject({ code: "INVALID_INPUT" });
  });
});

describe("mock apply_ddl", () => {
  it("updates the catalog: column rename / add / drop, index, rename table", async () => {
    const c = await connect("p-local-pg");
    const result = await ipc.ddl.apply(c, PG_USERS, [
      { op: "renameColumn", table: PG_USERS, name: "nickname", newName: "nick" },
      { op: "dropColumn", table: PG_USERS, name: "avatar" },
      {
        op: "addColumn",
        table: PG_USERS,
        column: {
          name: "slug",
          dataType: "text",
          nullable: true,
          defaultExpr: null,
          autoIncrement: false,
          comment: null,
          collation: null,
          unsigned: false,
          onUpdate: null,
        },
        after: null,
      },
      {
        op: "addIndex",
        table: PG_USERS,
        index: {
          name: "users_slug_idx",
          columns: ["slug"],
          unique: true,
          method: null,
          predicate: null,
          comment: null,
        },
      },
    ]);
    expect(result.error).toBeNull();
    expect(result.executed).toHaveLength(4);
    const meta = await ipc.schema.table(c, PG_USERS);
    const names = meta.columns.map((x) => x.name);
    expect(names).toContain("nick");
    expect(names).not.toContain("nickname");
    expect(names).not.toContain("avatar");
    expect(names.at(-1)).toBe("slug");
    expect(meta.indexes.map((i) => i.name)).toContain("users_slug_idx");
    // Rows stay in shape with the new column list.
    const page = await ipc.browse.rows(c, {
      table: PG_USERS,
      columns: null,
      filter: null,
      sort: [],
      limit: 5,
      offset: 0,
    });
    expect(page.rows[0]).toHaveLength(meta.columns.length);

    const t = { schema: "public", name: "tags" };
    await ipc.ddl.apply(c, t, [{ op: "renameTable", table: t, newName: "labels" }]);
    const tables = (await ipc.schema.tables(c, "public")).map((x) => x.name);
    expect(tables).toContain("labels");
    expect(tables).not.toContain("tags");
    await expect(ipc.schema.table(c, t)).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("replaces, drops and adds the primary key", async () => {
    const c = await connect("p-local-pg");
    const before = await ipc.schema.table(c, PG_USERS);
    expect(before.primaryKey).toEqual(["id"]);
    const plan = await ipc.ddl.preview(c, PG_USERS, [
      { op: "dropPrimaryKey", table: PG_USERS },
      { op: "addPrimaryKey", table: PG_USERS, columns: ["id", "email"] },
    ]);
    expect(plan.statements).toEqual([
      'ALTER TABLE "public"."users" DROP CONSTRAINT "users_pkey"',
      'ALTER TABLE "public"."users" ADD PRIMARY KEY ("id", "email")',
    ]);
    let result = await ipc.ddl.apply(c, PG_USERS, [
      { op: "dropPrimaryKey", table: PG_USERS },
      { op: "addPrimaryKey", table: PG_USERS, columns: ["id", "email"] },
    ]);
    expect(result.error).toBeNull();
    let meta = await ipc.schema.table(c, PG_USERS);
    expect(meta.primaryKey).toEqual(["id", "email"]);
    expect(meta.columns.filter((x) => x.isPk).map((x) => x.name)).toEqual(["id", "email"]);

    result = await ipc.ddl.apply(c, PG_USERS, [{ op: "dropPrimaryKey", table: PG_USERS }]);
    expect(result.error).toBeNull();
    meta = await ipc.schema.table(c, PG_USERS);
    expect(meta.primaryKey).toEqual([]);
    expect(meta.columns.some((x) => x.isPk)).toBe(false);

    result = await ipc.ddl.apply(c, PG_USERS, [
      { op: "addPrimaryKey", table: PG_USERS, columns: ["email"] },
    ]);
    expect(result.error).toBeNull();
    meta = await ipc.schema.table(c, PG_USERS);
    expect(meta.primaryKey).toEqual(["email"]);
  });

  it("fails on an unknown type: PG rolls back everything", async () => {
    const c = await connect("p-local-pg");
    const result = await ipc.ddl.apply(c, PG_USERS, [
      { op: "renameColumn", table: PG_USERS, name: "bio", newName: "about" },
      { op: "alterColumnType", table: PG_USERS, name: "about", newType: "nosuchtype", using: null },
    ]);
    expect(result.failedIndex).toBe(1);
    expect(result.executed).toEqual([]);
    expect(result.error).toMatchObject({
      message: 'type "nosuchtype" does not exist',
      sqlstate: "42704",
      statementIndex: 1,
    });
    const meta = await ipc.schema.table(c, PG_USERS);
    expect(meta.columns.map((x) => x.name)).toContain("bio");
  });

  it("fails on an unknown type: MySQL keeps the statements that ran", async () => {
    const c = await connect("p-shop-mysql");
    const t = { schema: "tabler_dev", name: "users" };
    const result = await ipc.ddl.apply(c, t, [
      { op: "renameColumn", table: t, name: "bio", newName: "about" },
      { op: "alterColumnType", table: t, name: "about", newType: "nosuchtype", using: null },
    ]);
    expect(result.failedIndex).toBe(1);
    expect(result.executed).toEqual([
      "ALTER TABLE `tabler_dev`.`users` RENAME COLUMN `bio` TO `about`",
    ]);
    expect(result.error?.vendorCode).toBe(1064);
    const meta = await ipc.schema.table(c, t);
    expect(meta.columns.map((x) => x.name)).toContain("about");
  });

  it("creates and drops tables and views", async () => {
    const c = await connect("p-local-pg");
    const t = { schema: "public", name: "notes" };
    await ipc.ddl.apply(c, t, [
      {
        op: "createTable",
        table: t,
        columns: [
          {
            name: "id",
            dataType: "bigint",
            nullable: false,
            defaultExpr: null,
            autoIncrement: true,
            comment: null,
            collation: null,
            unsigned: false,
            onUpdate: null,
          },
        ],
        primaryKey: ["id"],
        ifNotExists: false,
      },
    ]);
    expect((await ipc.schema.tables(c, "public")).map((x) => x.name)).toContain("notes");
    expect((await ipc.schema.table(c, t)).primaryKey).toEqual(["id"]);
    await ipc.ddl.apply(c, null, [{ op: "rawSql", sql: 'DROP VIEW "public"."active_users"' }]);
    await ipc.ddl.apply(c, t, [{ op: "dropTable", table: t, ifExists: false, cascade: false }]);
    const names = (await ipc.schema.tables(c, "public")).map((x) => x.name);
    expect(names).not.toContain("notes");
    expect(names).not.toContain("active_users");
  });

  it("creating or renaming to a name the schema already has fails like the server", async () => {
    const c = await connect("p-local-pg");
    const tags = { schema: "public", name: "tags" };
    const before = await ipc.schema.table(c, tags);
    const create = await ipc.ddl.apply(c, tags, [
      {
        op: "createTable",
        table: tags,
        columns: [
          {
            name: "id",
            dataType: "bigint",
            nullable: false,
            defaultExpr: null,
            autoIncrement: false,
            comment: null,
            collation: null,
            unsigned: false,
            onUpdate: null,
          },
        ],
        primaryKey: [],
        ifNotExists: false,
      },
    ]);
    expect(create.error).toMatchObject({
      message: 'relation "tags" already exists',
      sqlstate: "42P07",
      statementIndex: 0,
    });
    expect(create.failedIndex).toBe(0);
    expect(await ipc.schema.table(c, tags)).toEqual(before);

    const rename = await ipc.ddl.apply(c, tags, [
      { op: "renameTable", table: tags, newName: "posts" },
    ]);
    expect(rename.error?.message).toBe('relation "posts" already exists');
    const names = (await ipc.schema.tables(c, "public")).map((x) => x.name);
    expect(names.filter((n) => n === "posts")).toHaveLength(1);
    expect(names).toContain("tags");

    const m = await connect("p-shop-mysql");
    const mt = { schema: "tabler_dev", name: "tags" };
    const r = await ipc.ddl.apply(m, mt, [{ op: "renameTable", table: mt, newName: "users" }]);
    expect(r.error).toMatchObject({ message: "Table 'users' already exists", vendorCode: 1050 });
  });

  it("keeps foreign keys in step with renamed and dropped columns and tables", async () => {
    const c = await connect("p-local-pg");
    const posts = { schema: "public", name: "posts" };
    const fkOf = async (t: TableRef) =>
      (await ipc.schema.table(c, t)).foreignKeys.find((f) => f.name === "posts_user_id_fkey");

    await ipc.ddl.apply(c, posts, [
      { op: "renameColumn", table: posts, name: "user_id", newName: "author_id" },
    ]);
    expect(await fkOf(posts)).toMatchObject({ columns: ["author_id"], refColumns: ["id"] });

    await ipc.ddl.apply(c, PG_USERS, [
      { op: "renameColumn", table: PG_USERS, name: "id", newName: "user_id" },
    ]);
    expect(await fkOf(posts)).toMatchObject({ refColumns: ["user_id"] });

    await ipc.ddl.apply(c, PG_USERS, [{ op: "renameTable", table: PG_USERS, newName: "people" }]);
    expect((await fkOf(posts))?.refTable).toEqual({ schema: "public", name: "people" });

    await ipc.ddl.apply(c, posts, [{ op: "dropColumn", table: posts, name: "author_id" }]);
    expect(await fkOf(posts)).toBeUndefined();
  });

  it("refuses to run on a read-only connection", async () => {
    const c = await connect("p-prod-pg");
    await expect(
      ipc.ddl.apply(c, PG_USERS, [{ op: "dropColumn", table: PG_USERS, name: "bio" }]),
    ).rejects.toMatchObject({ code: "UNSUPPORTED" });
  });
});

describe("mock get_table_ddl", () => {
  it("builds CREATE TABLE from the metadata per dialect", async () => {
    const pg = await connect("p-local-pg");
    const posts = { schema: "public", name: "posts" };
    const ddl = await ipc.ddl.tableDdl(pg, posts);
    expect(ddl).toMatch(/^CREATE TABLE "public"\."posts" \(/);
    expect(ddl).toContain('"id" bigint GENERATED ALWAYS AS IDENTITY NOT NULL');
    expect(ddl).toContain('CONSTRAINT "posts_pkey" PRIMARY KEY ("id")');
    expect(ddl).toContain(
      'FOREIGN KEY ("user_id") REFERENCES "public"."users" ("id") ON DELETE CASCADE',
    );
    expect(ddl).toContain("CREATE TRIGGER posts_set_updated_at");

    const my = await connect("p-shop-mysql");
    const myDdl = await ipc.ddl.tableDdl(my, { schema: "tabler_dev", name: "tags" });
    expect(myDdl).toContain("CREATE TABLE `tabler_dev`.`tags`");
    expect(myDdl).toContain("ENGINE=InnoDB");
  });
});

describe("mock checks, triggers, table options and choices", () => {
  it("applies CHECK and trigger changes to the catalog", async () => {
    const c = await connect("p-local-pg");
    const before = await ipc.schema.table(c, PG_USERS);
    expect(before.checks).toEqual([
      { name: "users_balance_check", expression: "balance >= 0", columns: ["balance"] },
    ]);
    const ops: DdlOp[] = [
      { op: "dropCheck", table: PG_USERS, name: "users_balance_check" },
      { op: "addCheck", table: PG_USERS, check: { name: "score_ok", expression: "score < 10" } },
      {
        op: "createTrigger",
        table: PG_USERS,
        name: "users_audit",
        sql: "CREATE TRIGGER users_audit AFTER DELETE ON users FOR EACH ROW EXECUTE FUNCTION audit();",
      },
    ];
    const plan = await ipc.ddl.preview(c, PG_USERS, ops);
    expect(plan.statements).toEqual([
      'ALTER TABLE "public"."users" DROP CONSTRAINT "users_balance_check"',
      'ALTER TABLE "public"."users" ADD CONSTRAINT "score_ok" CHECK (score < 10)',
      "CREATE TRIGGER users_audit AFTER DELETE ON users FOR EACH ROW EXECUTE FUNCTION audit()",
    ]);
    expect((await ipc.ddl.apply(c, PG_USERS, ops)).error).toBeNull();
    const after = await ipc.schema.table(c, PG_USERS);
    expect(after.checks).toEqual([
      { name: "score_ok", expression: "score < 10", columns: ["score"] },
    ]);
    expect(after.triggers.map((t) => [t.name, t.timing, t.events])).toEqual([
      ["users_audit", "AFTER", ["DELETE"]],
    ]);
  });

  it("MySQL: options and ON UPDATE; the pickers' choices per driver", async () => {
    const c = await connect("p-shop-mysql");
    const t = { schema: "tabler_dev", name: "users" };
    const meta = await ipc.schema.table(c, t);
    expect(meta.options).toMatchObject({ engine: "InnoDB", charset: "utf8mb4" });
    const ops: DdlOp[] = [
      {
        op: "alterTableOptions",
        table: t,
        options: {
          engine: null,
          charset: "latin1",
          collation: null,
          autoIncrement: 500,
          rowFormat: null,
        },
      },
      { op: "alterColumnOnUpdate", table: t, name: "last_login", onUpdate: "CURRENT_TIMESTAMP" },
    ];
    const plan = await ipc.ddl.preview(c, t, ops);
    expect(plan.statements[0]).toBe(
      "ALTER TABLE `tabler_dev`.`users` CONVERT TO CHARACTER SET latin1, AUTO_INCREMENT = 500",
    );
    expect(plan.statements[1]).toMatch(
      /MODIFY COLUMN `last_login` .* ON UPDATE CURRENT_TIMESTAMP$/,
    );
    await ipc.ddl.apply(c, t, ops);
    const after = await ipc.schema.table(c, t);
    expect(after.options).toMatchObject({ charset: "latin1", autoIncrement: 500 });
    expect(after.columns.find((x) => x.name === "last_login")?.extra).toBe(
      "on update CURRENT_TIMESTAMP",
    );

    expect((await ipc.schema.structureChoices(c)).engines).toContain("InnoDB");
    const pg = await connect("p-local-pg");
    expect((await ipc.schema.structureChoices(pg)).userTypes).toContainEqual({
      name: "user_role",
      kind: "enum",
    });
  });
});
