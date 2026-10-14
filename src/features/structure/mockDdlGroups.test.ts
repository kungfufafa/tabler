import { afterEach, describe, expect, it } from "vitest";
import { ipc, type ColumnDef, type DdlOp, type TableRef } from "@/lib/ipc";
import { resetMockDdl } from "@/lib/ipc/mock/ddl";

// The mock planner groups the clauses of one table into one ALTER TABLE like `sqlgen/ddl.rs`
// (MySQL and PostgreSQL), so the browser mock and the real planner agree.

const PG: TableRef = { schema: "public", name: "users" };
const MY: TableRef = { schema: "tabler_dev", name: "users" };

afterEach(() => resetMockDdl());

const connect = async (profileId: string) =>
  (await ipc.connections.connect(profileId)).connectionId;

const def = (name: string, dataType = "text", extra: Partial<ColumnDef> = {}): ColumnDef => ({
  name,
  dataType,
  nullable: true,
  defaultExpr: null,
  autoIncrement: false,
  comment: null,
  collation: null,
  unsigned: false,
  onUpdate: null,
  ...extra,
});

const add = (
  table: TableRef,
  name: string,
  dataType?: string,
  extra?: Partial<ColumnDef>,
): DdlOp => ({
  op: "addColumn",
  table,
  column: def(name, dataType, extra),
  after: null,
});

describe("mock preview_ddl: grouped ALTER TABLE", () => {
  it("PostgreSQL and MySQL: two adds and a drop of one table are one statement", async () => {
    const ops = (t: TableRef): DdlOp[] => [
      add(t, "a", "int"),
      add(t, "b"),
      { op: "dropColumn", table: t, name: "avatar" },
    ];
    const pg = await ipc.ddl.preview(await connect("p-local-pg"), PG, ops(PG));
    expect(pg.statements).toEqual([
      'ALTER TABLE "public"."users" ADD COLUMN "a" int, ADD COLUMN "b" text, DROP COLUMN "avatar"',
    ]);
    expect(pg.transactional).toBe(true);
    const my = await ipc.ddl.preview(await connect("p-shop-mysql"), MY, ops(MY));
    expect(my.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` ADD COLUMN `a` int NULL, ADD COLUMN `b` text NULL, DROP COLUMN `avatar`",
    ]);
    // one statement fails as a whole: all or nothing
    expect(my.transactional).toBe(true);
  });

  it("an added CHECK and the drop of a column in the same plan are two statements, in order", async () => {
    const check = (t: TableRef): DdlOp => ({
      op: "addCheck",
      table: t,
      check: { name: "ck_nick", expression: "length(nickname) > 1" },
    });
    const drop = (t: TableRef): DdlOp => ({ op: "dropColumn", table: t, name: "nickname" });
    for (const [profile, t] of [
      ["p-local-pg", PG],
      ["p-shop-mysql", MY],
    ] as const) {
      const c = await connect(profile);
      for (const ops of [
        [check(t), drop(t)],
        [drop(t), check(t)],
      ]) {
        const plan = await ipc.ddl.preview(c, t, ops);
        expect(plan.statements, profile).toHaveLength(2);
      }
    }
  });

  it("a MySQL plan of several statements is not transactional", async () => {
    const c = await connect("p-shop-mysql");
    const plan = await ipc.ddl.preview(c, MY, [
      add(MY, "a", "int"),
      { op: "renameColumn", table: MY, name: "nickname", newName: "nick" },
      add(MY, "b", "int"),
    ]);
    expect(plan.statements).toHaveLength(3);
    expect(plan.transactional).toBe(false);
  });

  it("drop then add of one column keeps its order; add then drop cannot share a statement", async () => {
    const pg = await connect("p-local-pg");
    expect(
      (
        await ipc.ddl.preview(pg, PG, [
          { op: "dropColumn", table: PG, name: "bio" },
          add(PG, "bio", "varchar(20)"),
        ])
      ).statements,
    ).toEqual(['ALTER TABLE "public"."users" DROP COLUMN "bio", ADD COLUMN "bio" varchar(20)']);
    for (const profile of ["p-local-pg", "p-shop-mysql"]) {
      const t = profile === "p-local-pg" ? PG : MY;
      const plan = await ipc.ddl.preview(await connect(profile), t, [
        add(t, "tmp", "int"),
        { op: "dropColumn", table: t, name: "tmp" },
      ]);
      expect(plan.statements).toHaveLength(2);
    }
  });

  it("MySQL: a column a key covers is dropped and added again in two statements (1089 / 1171)", async () => {
    // the server refuses `DROP COLUMN id, ADD COLUMN id` while the primary key covers it
    const c = await connect("p-shop-mysql");
    const swap = (name: string): DdlOp[] => [
      { op: "dropColumn", table: MY, name },
      add(MY, name, "bigint"),
    ];
    const keyed = await ipc.ddl.preview(c, MY, swap("id"));
    expect(keyed.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` DROP COLUMN `id`",
      "ALTER TABLE `tabler_dev`.`users` ADD COLUMN `id` bigint NULL",
    ]);
    expect(keyed.transactional).toBe(false);
    // a column of no key keeps the single statement
    const free = await ipc.ddl.preview(c, MY, swap("bio"));
    expect(free.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` DROP COLUMN `bio`, ADD COLUMN `bio` bigint NULL",
    ]);
    // PostgreSQL drops the key with the column: one statement
    const pg = await ipc.ddl.preview(await connect("p-local-pg"), PG, [
      { op: "dropColumn", table: PG, name: "id" },
      add(PG, "id", "bigint"),
    ]);
    expect(pg.statements).toHaveLength(1);
  });

  it("MySQL: a foreign key dropped and added under one name is two statements (1826); PostgreSQL joins them", async () => {
    const fk = (t: TableRef): DdlOp => ({
      op: "addForeignKey",
      table: t,
      fk: {
        name: "fk_nick",
        columns: ["nickname"],
        refTable: t,
        refColumns: ["id"],
        onUpdate: null,
        onDelete: "CASCADE",
      },
    });
    const ops = (t: TableRef): DdlOp[] => [
      fk(t),
      { op: "dropForeignKey", table: t, name: "fk_nick" },
      fk(t),
    ];
    // (the first add and the drop cannot share a statement anywhere)
    const my = await ipc.ddl.preview(await connect("p-shop-mysql"), MY, ops(MY));
    expect(my.statements).toHaveLength(3);
    expect(my.statements[1]).toBe("ALTER TABLE `tabler_dev`.`users` DROP FOREIGN KEY `fk_nick`");
    const pg = await ipc.ddl.preview(await connect("p-local-pg"), PG, ops(PG));
    expect(pg.statements).toHaveLength(2);
    expect(pg.statements[1]).toContain('DROP CONSTRAINT "fk_nick", ADD CONSTRAINT "fk_nick"');
  });

  it("MySQL: a second change of a column replaces the first MODIFY; a new column is not MODIFIED in its statement", async () => {
    const c = await connect("p-shop-mysql");
    const plan = await ipc.ddl.preview(c, MY, [
      { op: "alterColumnNullable", table: MY, name: "email", nullable: true },
      { op: "alterColumnDefault", table: MY, name: "email", defaultExpr: "'x'" },
      { op: "alterColumnType", table: MY, name: "bio", newType: "mediumtext", using: null },
    ]);
    expect(plan.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` MODIFY COLUMN `email` text NULL DEFAULT 'x', MODIFY COLUMN `bio` mediumtext NULL",
    ]);
    const split = await ipc.ddl.preview(c, MY, [
      add(MY, "tag", "varchar(10)"),
      { op: "alterColumnNullable", table: MY, name: "tag", nullable: false },
    ]);
    expect(split.statements).toHaveLength(2);
    expect(split.statements[1]).toContain("MODIFY COLUMN `tag` varchar(10) NOT NULL");
  });

  it("PostgreSQL: a type change never shares a statement with another clause on its column", async () => {
    const c = await connect("p-local-pg");
    const plan = await ipc.ddl.preview(c, PG, [
      { op: "alterColumnType", table: PG, name: "bio", newType: "varchar(40)", using: null },
      { op: "alterColumnDefault", table: PG, name: "bio", defaultExpr: "'x'" },
      { op: "alterColumnNullable", table: PG, name: "bio", nullable: false },
      { op: "alterColumnDefault", table: PG, name: "nickname", defaultExpr: "'n'" },
    ]);
    expect(plan.statements).toEqual([
      'ALTER TABLE "public"."users" ALTER COLUMN "bio" TYPE varchar(40)',
      `ALTER TABLE "public"."users" ALTER COLUMN "bio" SET DEFAULT 'x', ALTER COLUMN "bio" SET NOT NULL, ALTER COLUMN "nickname" SET DEFAULT 'n'`,
    ]);
    // other columns share it before and after
    const mixed = await ipc.ddl.preview(c, PG, [
      { op: "alterColumnNullable", table: PG, name: "nickname", nullable: false },
      { op: "alterColumnType", table: PG, name: "bio", newType: "varchar(40)", using: null },
      { op: "alterColumnDefault", table: PG, name: "nickname", defaultExpr: "'n'" },
    ]);
    expect(mixed.statements).toHaveLength(1);
  });

  it("statements of their own end the ALTER: comments, CREATE INDEX, raw SQL, a rename", async () => {
    const c = await connect("p-local-pg");
    const plan = await ipc.ddl.preview(c, PG, [
      add(PG, "a", "int", { comment: "first" }),
      add(PG, "b", "int"),
      {
        op: "addIndex",
        table: PG,
        index: {
          name: "users_b_idx",
          columns: ["b"],
          unique: false,
          method: null,
          predicate: null,
          comment: null,
          include: [],
        },
      },
      add(PG, "c", "int"),
      { op: "rawSql", sql: "ANALYZE users" },
      add(PG, "d", "int"),
    ]);
    expect(plan.statements).toEqual([
      'ALTER TABLE "public"."users" ADD COLUMN "a" int',
      `COMMENT ON COLUMN "public"."users"."a" IS 'first'`,
      'ALTER TABLE "public"."users" ADD COLUMN "b" int',
      'CREATE INDEX "users_b_idx" ON "public"."users" ("b")',
      'ALTER TABLE "public"."users" ADD COLUMN "c" int',
      "ANALYZE users",
      'ALTER TABLE "public"."users" ADD COLUMN "d" int',
    ]);
  });

  it("MySQL: indexes and checks are clauses; a lone index stays CREATE INDEX", async () => {
    const c = await connect("p-shop-mysql");
    const index = (name: string, columns: string[]) =>
      ({
        op: "addIndex",
        table: MY,
        index: {
          name,
          columns,
          unique: false,
          method: null,
          predicate: null,
          comment: null,
          include: [],
        },
      }) satisfies DdlOp;
    const lone = await ipc.ddl.preview(c, MY, [index("users_email_ix", ["email(10)"])]);
    expect(lone.statements).toEqual([
      "CREATE INDEX `users_email_ix` ON `tabler_dev`.`users` (`email`(10))",
    ]);
    const group = await ipc.ddl.preview(c, MY, [
      add(MY, "code", "varchar(12)"),
      index("users_code_ix", ["code"]),
      { op: "addCheck", table: MY, check: { name: "score_ok", expression: "score >= 0" } },
    ]);
    expect(group.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` ADD COLUMN `code` varchar(12) NULL, ADD INDEX `users_code_ix` (`code`), ADD CONSTRAINT `score_ok` CHECK (score >= 0)",
    ]);
  });

  it("MySQL refuses a TEXT / BLOB column in a plain index without a prefix length", async () => {
    const c = await connect("p-shop-mysql");
    const op = (columns: string[], method: string | null = null): DdlOp => ({
      op: "addIndex",
      table: MY,
      index: {
        name: "users_email_ix",
        columns,
        unique: false,
        method,
        predicate: null,
        comment: null,
        include: [],
      },
    });
    await expect(ipc.ddl.preview(c, MY, [op(["email"])])).rejects.toMatchObject({
      code: "INVALID_INPUT",
      message: expect.stringMatching(/prefix length.*email\(10\)/),
    });
    await expect(ipc.ddl.preview(c, MY, [op(["email(10)"])])).resolves.toBeTruthy();
    // FULLTEXT takes whole columns
    await expect(ipc.ddl.preview(c, MY, [op(["email"], "FULLTEXT")])).resolves.toBeTruthy();
    // other databases take a TEXT column whole
    const pg = await connect("p-local-pg");
    await expect(
      ipc.ddl.preview(pg, PG, [
        { ...(op(["email"]) as Extract<DdlOp, { op: "addIndex" }>), table: PG },
      ]),
    ).resolves.toBeTruthy();
  });

  it("MySQL: CURRENT_TIMESTAMP takes the precision of the column", async () => {
    const c = await connect("p-shop-mysql");
    const plan = await ipc.ddl.preview(c, MY, [
      add(MY, "seen", "datetime(3)", {
        defaultExpr: "CURRENT_TIMESTAMP",
        onUpdate: "CURRENT_TIMESTAMP",
      }),
      add(MY, "plain", "datetime", { onUpdate: "CURRENT_TIMESTAMP(3)" }),
    ]);
    expect(plan.statements).toEqual([
      "ALTER TABLE `tabler_dev`.`users` ADD COLUMN `seen` datetime(3) NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3), ADD COLUMN `plain` datetime NULL ON UPDATE CURRENT_TIMESTAMP",
    ]);
  });
});

describe("mock apply_ddl: a grouped statement is all or nothing", () => {
  it("MySQL: a refused clause leaves the table as it was and nothing is executed", async () => {
    const c = await connect("p-shop-mysql");
    const ops: DdlOp[] = [
      { op: "alterColumnNullable", table: MY, name: "email", nullable: true },
      { op: "alterColumnType", table: MY, name: "bio", newType: "nosuchtype", using: null },
    ];
    expect((await ipc.ddl.preview(c, MY, ops)).transactional).toBe(true);
    const result = await ipc.ddl.apply(c, MY, ops);
    expect(result.error).not.toBeNull();
    expect(result.failedIndex).toBe(0);
    // nothing ran: "All changes were reverted"
    expect(result.executed).toEqual([]);
    const meta = await ipc.schema.table(c, MY);
    expect(meta.columns.find((x) => x.name === "email")!.nullable).toBe(false);
    expect(meta.columns.find((x) => x.name === "bio")!.dataType).not.toBe("nosuchtype");
  });

  it("MySQL: with several statements the ones before the failing one stay applied", async () => {
    const c = await connect("p-shop-mysql");
    const ops: DdlOp[] = [
      { op: "renameColumn", table: MY, name: "nickname", newName: "nick" },
      { op: "alterColumnType", table: MY, name: "bio", newType: "nosuchtype", using: null },
    ];
    expect((await ipc.ddl.preview(c, MY, ops)).transactional).toBe(false);
    const result = await ipc.ddl.apply(c, MY, ops);
    expect(result.failedIndex).toBe(1);
    expect(result.executed).toHaveLength(1);
    const meta = await ipc.schema.table(c, MY);
    expect(meta.columns.map((x) => x.name)).toContain("nick");
  });

  it("a grouped plan applies every clause to the catalog", async () => {
    const c = await connect("p-local-pg");
    const result = await ipc.ddl.apply(c, PG, [
      add(PG, "slug", "text"),
      { op: "dropColumn", table: PG, name: "avatar" },
      { op: "alterColumnNullable", table: PG, name: "nickname", nullable: false },
    ]);
    expect(result.error).toBeNull();
    expect(result.executed).toHaveLength(1);
    const meta = await ipc.schema.table(c, PG);
    const names = meta.columns.map((x) => x.name);
    expect(names).toContain("slug");
    expect(names).not.toContain("avatar");
    expect(meta.columns.find((x) => x.name === "nickname")!.nullable).toBe(false);
  });
});
