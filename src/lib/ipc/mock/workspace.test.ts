import { afterEach, describe, expect, it } from "vitest";
import {
  ipc,
  type RoutineRef,
  type ServerExpr,
  type TableRef,
  type WorkspaceGroup,
} from "@/lib/ipc";
import { resetMockDdl } from "./ddl";
import { resetMockObjects } from "./objects";

const USERS: TableRef = { schema: "public", name: "users" };

async function connect(profileId = "p-local-pg") {
  return (await ipc.connections.connect(profileId)).connectionId;
}

afterEach(() => {
  resetMockDdl();
  resetMockObjects();
});

/** An update of a timestamp column with the server's NOW() and of the name with a value. */
const rows = (): WorkspaceGroup => ({
  kind: "rows",
  request: {
    table: USERS,
    returning: true,
    changes: [
      {
        op: "update",
        rowId: "r1",
        key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
        changes: { name: { t: "text", v: "Ada" } },
        exprs: { last_login: "now" },
      },
    ],
  },
});

const ddl: WorkspaceGroup = {
  kind: "ddl",
  table: USERS,
  ops: [{ op: "setTableComment", table: USERS, comment: "people" }],
};

const object: WorkspaceGroup = {
  kind: "object",
  op: {
    op: "truncateTables",
    tables: [{ schema: "public", name: "tags" }],
    cascade: false,
    restartIdentity: false,
    ignoreForeignKeys: false,
  },
};

describe("mock workspace commit", () => {
  it("previews each group with the SQL of its own command, the server's NOW() unquoted", async () => {
    const c = await connect();
    const plan = await ipc.workspace.preview(c, [rows(), ddl, object]);
    expect(plan.atomic).toBe(true);
    expect(plan.groups).toHaveLength(3);
    expect(plan.groups.every((g) => g.error === null)).toBe(true);
    const [update, comment, truncate] = plan.groups;
    expect(update!.statements[0]).toMatch(/^UPDATE "public"\."users" SET .*"last_login" = NOW\(\)/);
    expect(comment!.statements.join(" ")).toMatch(/COMMENT ON TABLE/);
    expect(truncate!.statements.join(" ")).toMatch(/TRUNCATE/);
  });

  it("previews a definition as DROP … IF EXISTS plus the text, and refuses anything but CREATE", async () => {
    const c = await connect();
    const drop = {
      schema: "public",
      name: "slugify",
      kind: "function" as const,
      identityArguments: "value text",
    };
    const plan = await ipc.workspace.preview(c, [
      {
        kind: "definition",
        drop,
        sql: "CREATE OR REPLACE FUNCTION public.slugify(value text) RETURNS text AS $$ SELECT value $$ LANGUAGE sql;",
      },
      { kind: "definition", drop: null, sql: "DROP TABLE users" },
    ]);
    expect(plan.groups[0]!.statements).toEqual([
      'DROP FUNCTION IF EXISTS "public"."slugify"(value text)',
      "CREATE OR REPLACE FUNCTION public.slugify(value text) RETURNS text AS $$ SELECT value $$ LANGUAGE sql",
    ]);
    expect(plan.groups[1]!.error?.message).toMatch(/starts with CREATE/);
    expect(plan.atomic).toBe(false);
  });

  it("previews a typed precision the way the backend renders it, per dialect", async () => {
    const update = (exprs: Record<string, ServerExpr>): WorkspaceGroup => ({
      kind: "rows",
      request: {
        table: USERS,
        returning: false,
        changes: [
          {
            op: "update",
            rowId: "r1",
            key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
            changes: {},
            exprs,
          },
        ],
      },
    });
    const pg = await connect();
    const my = await connect("p-shop-mysql");
    const sql = async (c: string, exprs: Record<string, ServerExpr>) =>
      (await ipc.workspace.preview(c, [update(exprs)])).groups[0]!.statements[0]!;
    // MySQL keeps the digits on every clock (a bare CURRENT_TIMESTAMP stores none).
    expect(
      await sql(my, { a: "currentTimestamp(6)", b: "now(3)", c: "currentTime(1)", d: "now(6)" }),
    ).toMatch(
      /"a" = CURRENT_TIMESTAMP\(6\), "b" = NOW\(3\), "c" = CURRENT_TIME\(1\), "d" = NOW\(6\) /,
    );
    // PostgreSQL's now() has no argument: NOW(n) is its CURRENT_TIMESTAMP(n).
    expect(await sql(pg, { a: "now(6)", b: "localTimestamp(2)" })).toMatch(
      /"a" = CURRENT_TIMESTAMP\(6\), "b" = LOCALTIMESTAMP\(2\) /,
    );
    // Without a precision nothing changes.
    expect(await sql(my, { a: "now", b: "currentTimestamp" })).toMatch(
      /"a" = NOW\(\), "b" = CURRENT_TIMESTAMP /,
    );
  });

  it("stores a clock with as many fractional digits as the precision asks", async () => {
    const c = await connect();
    const result = await ipc.workspace.apply(c, [
      {
        kind: "rows",
        request: {
          table: USERS,
          returning: true,
          changes: [
            {
              op: "update",
              rowId: "r1",
              key: { kind: "pk", value: { id: { t: "int", v: 1 } } },
              changes: {},
              exprs: { last_login: "now(6)" },
            },
          ],
        },
      },
    ]);
    expect(result.success).toBe(true);
    const stored = result.groups[0]!.rows[0]!.returned?.last_login;
    expect(stored).toMatchObject({ t: "timestamp" });
    expect(stored && stored.t === "timestamp" && stored.v).toMatch(/\d\d:\d\d:\d\d\.\d{6}$/);
  });

  it("saves a MySQL routine as opened (DROP … IF EXISTS in front of the CREATE) and warns about the drop", async () => {
    const c = await connect("p-shop-mysql");
    const opened = await ipc.objects.routineDefinition(c, {
      schema: "shop",
      name: "calc_total",
      kind: "function",
      identityArguments: "",
    });
    expect(opened).toMatch(/^DROP FUNCTION IF EXISTS /);
    const drop: RoutineRef = {
      schema: "shop",
      name: "calc_total",
      kind: "function",
      identityArguments: "",
    };
    const plan = await ipc.workspace.preview(c, [{ kind: "definition", drop, sql: opened }]);
    const group = plan.groups[0]!;
    expect(group.error).toBeNull();
    // One DROP (the plan's own, qualified), then the CREATE: the DROP of the text is not run twice.
    expect(group.statements).toHaveLength(2);
    expect(group.statements[0]).toBe("DROP FUNCTION IF EXISTS `shop`.`calc_total`");
    // Created in the schema of the routine it replaces (the connection may have another selected).
    expect(group.statements[1]).toMatch(
      /^CREATE DEFINER=`root`@`localhost` FUNCTION `shop`\.`calc_total`\(/,
    );
    expect(group.statements[1]).not.toMatch(/;$/);
    expect(group.warnings.join(" ")).toMatch(/cannot replace a routine in one step/);
    expect(group.transactional).toBe(false);
    const applied = await ipc.workspace.apply(c, [{ kind: "definition", drop, sql: opened }]);
    expect(applied.success).toBe(true);
    expect(applied.groups[0]!.routine).toMatchObject({ name: "calc_total", kind: "function" });
    // A DROP of another routine is refused, and a tab without a routine takes the typed DROP.
    const other = await ipc.workspace.preview(c, [
      {
        kind: "definition",
        drop,
        sql: "DROP FUNCTION IF EXISTS other_fn;\nCREATE FUNCTION calc_total() RETURNS int RETURN 1",
      },
      {
        kind: "definition",
        drop: null,
        sql: "DROP PROCEDURE IF EXISTS refresh;\nCREATE PROCEDURE refresh() SELECT 1",
      },
    ]);
    expect(other.groups[0]!.error?.message).toMatch(/names other_fn, but this tab replaces/);
    expect(other.groups[1]!.statements[0]).toBe("DROP PROCEDURE IF EXISTS `refresh`");
  });

  it("applies the groups in TablePlus's order, reporting each in request order", async () => {
    const c = await connect();
    const result = await ipc.workspace.apply(c, [object, ddl, rows()]);
    expect(result.success).toBe(true);
    expect(result.atomic).toBe(true);
    expect(result.groups.map((g) => g.status)).toEqual(["applied", "applied", "applied"]);
    expect(result.groups[2]!.rows[0]).toMatchObject({ rowId: "r1", op: "update" });
    // The structure change took: the table's comment follows.
    const meta = await ipc.schema.table(c, USERS, true);
    expect(meta.comment).toBe("people");
  });

  it("reports the routine a definition creates, for the tab's next save", async () => {
    const c = await connect();
    const result = await ipc.workspace.apply(c, [
      {
        kind: "definition",
        drop: null,
        sql: 'CREATE FUNCTION "analytics".Total_Of(IN price numeric DEFAULT 1) RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql',
      },
    ]);
    expect(result.groups[0]!.routine).toEqual({
      schema: "analytics",
      name: "total_of",
      kind: "function",
      identityArguments: "IN price numeric",
    });
  });

  it("stops at a failing group and says which one", async () => {
    const c = await connect();
    const bad: WorkspaceGroup = {
      kind: "ddl",
      table: USERS,
      ops: [{ op: "rawSql", sql: "SELECT nosuchtype_x" }],
    };
    const result = await ipc.workspace.apply(c, [rows(), bad, object]);
    expect(result.success).toBe(false);
    expect(result.failedGroup).toBe(1);
    expect(result.error?.message).toMatch(/nosuchtype_x/);
    expect(result.groups.map((g) => g.status)).toEqual(["applied", "failed", "notRun"]);
  });

  it("MySQL commits row edits in one transaction: atomic, and a failing edit reverts all of it", async () => {
    const my = await connect("p-shop-mysql");
    const missing: WorkspaceGroup = {
      kind: "rows",
      request: {
        table: USERS,
        returning: false,
        changes: [
          {
            op: "delete",
            rowId: "r9",
            key: { kind: "pk", value: { id: { t: "int", v: 999999 } } },
          },
        ],
      },
    };
    const badDdl: WorkspaceGroup = {
      kind: "ddl",
      table: USERS,
      ops: [{ op: "rawSql", sql: "SELECT nosuchtype_x" }],
    };

    // Preview: only row edits pending is one transaction; structure (DDL commits on its own) is not.
    expect((await ipc.workspace.preview(my, [rows()])).atomic).toBe(true);
    expect((await ipc.workspace.preview(my, [rows(), rows()])).atomic).toBe(true);
    expect((await ipc.workspace.preview(my, [rows(), ddl])).atomic).toBe(false);
    expect((await ipc.workspace.preview(my, [ddl])).atomic).toBe(false);

    // A rows-only commit that goes through is atomic ...
    const ok = await ipc.workspace.apply(my, [rows(), rows()]);
    expect(ok.success).toBe(true);
    expect(ok.atomic).toBe(true);
    // ... and one that fails says everything was reverted (the toast "All changes were reverted").
    const failed = await ipc.workspace.apply(my, [rows(), missing]);
    expect(failed.success).toBe(false);
    expect(failed.atomic).toBe(true);
    expect(failed.failedGroup).toBe(1);

    // With structure pending the commit is not one transaction ...
    const mixed = await ipc.workspace.apply(my, [rows(), ddl]);
    expect(mixed.success).toBe(true);
    expect(mixed.atomic).toBe(false);
    // ... but a failing row edit runs first and nothing else ran: atomic again.
    const early = await ipc.workspace.apply(my, [ddl, missing]);
    expect(early.success).toBe(false);
    expect(early.atomic).toBe(true);
    expect(early.groups.map((g) => g.status)).toEqual(["notRun", "failed"]);
    // A DDL that fails after the data edits were committed leaves them: not atomic.
    const late = await ipc.workspace.apply(my, [rows(), badDdl]);
    expect(late.success).toBe(false);
    expect(late.atomic).toBe(false);
    expect(late.groups.map((g) => g.status)).toEqual(["applied", "failed"]);
  });
});
