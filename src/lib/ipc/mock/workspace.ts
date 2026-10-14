// Mock handlers for the workspace commit: preview_workspace_changes and apply_workspace_changes.
// Each group goes through the mock of its own command (preview_changes / apply_changes,
// preview_ddl / apply_ddl, preview_object_op / apply_object_op), in TablePlus's order (data
// edits, then structure, then sidebar operations). The mock cannot roll a group back once a later
// one fails, so a failure stops the run with what ran before it applied.
//
// Atomicity follows the backend (`workspace_commit::is_atomic`): every plan transactional and, on
// MySQL (whose DDL commits on its own), nothing but row edits pending. A failing row edit leaves
// nothing applied even where the commit is not one transaction (the data edits run first, in a
// transaction of their own), so that failure is atomic too: "All changes were reverted".
import type { ApplyRequest } from "../bindings/ApplyRequest";
import type { ApplyResult } from "../bindings/ApplyResult";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { DdlPlan } from "../bindings/DdlPlan";
import type { DdlResult } from "../bindings/DdlResult";
import type { ErrorPayload } from "../bindings/ErrorPayload";
import type { GroupPlan } from "../bindings/GroupPlan";
import type { GroupResult } from "../bindings/GroupResult";
import type { RoutineRef } from "../bindings/RoutineRef";
import type { ServerExpr } from "../bindings/ServerExpr";
import type { StagedChange } from "../bindings/StagedChange";
import type { Value } from "../bindings/Value";
import type { WorkspaceGroup } from "../bindings/WorkspaceGroup";
import type { WorkspacePlan } from "../bindings/WorkspacePlan";
import type { WorkspaceResult } from "../bindings/WorkspaceResult";
import "./handlers";
import "./ddl";
import "./objects";
import { mockError, mockInvoke, registerMock } from "./transport";

/** A server expression as the keyword it is and the digits of its fractional-seconds precision. */
function splitExpr(e: ServerExpr): { name: string; fsp: number | null } {
  const m = /^(\w+)\((\d)\)$/.exec(e);
  return m ? { name: m[1]!, fsp: Number(m[2]) } : { name: e, fsp: null };
}

const KEYWORD: Record<string, string> = {
  now: "NOW",
  currentTimestamp: "CURRENT_TIMESTAMP",
  currentDate: "CURRENT_DATE",
  currentTime: "CURRENT_TIME",
  localTimestamp: "LOCALTIMESTAMP",
  default: "DEFAULT",
};

/**
 * The SQL of a server expression, as `sqlgen::mutate::server_expr_sql` renders it: a clock keeps
 * its precision (`NOW(6)`; PostgreSQL spells `now(n)` as `CURRENT_TIMESTAMP(n)`), SQLite has none.
 */
function exprSql(driver: string, e: ServerExpr): string {
  const { name, fsp } = splitExpr(e);
  if (name === "now" && fsp === null) return "NOW()";
  if (fsp === null || driver === "sqlite")
    return name === "now" ? KEYWORD.currentTimestamp! : KEYWORD[name]!;
  if (name === "now" && driver === "postgres") return `CURRENT_TIMESTAMP(${fsp})`;
  return `${KEYWORD[name]}(${fsp})`;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** What the mock stores for a server expression: the browser's clock in the usual forms. */
function exprValue(e: ServerExpr): Value {
  if (e === "default") return { t: "default" };
  const { name, fsp } = splitExpr(e);
  const at = new Date();
  const date = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
  // Fractional digits as the precision asks (the browser's clock stops at milliseconds).
  const fraction = fsp
    ? `.${String(at.getMilliseconds()).padStart(3, "0").padEnd(fsp, "0").slice(0, fsp)}`
    : "";
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}${fraction}`;
  if (name === "currentDate") return { t: "date", v: date };
  if (name === "currentTime") return { t: "time", v: time };
  return { t: "timestamp", v: `${date} ${time}` };
}

/**
 * `req` with its expression cells folded into plain values: for the preview as the keyword
 * (a decimal renders unquoted in the mutate mock), for the apply as the browser's clock.
 */
function folded(req: ApplyRequest, as: "sql" | "value", driver: string): ApplyRequest {
  const fold = (cells: Record<string, Value>, exprs: Record<string, ServerExpr> | undefined) => {
    const out = { ...cells };
    for (const [k, e] of Object.entries(exprs ?? {}))
      out[k] =
        as === "sql" && e !== "default" ? { t: "decimal", v: exprSql(driver, e) } : exprValue(e);
    return out;
  };
  return {
    ...req,
    changes: req.changes.map((c): StagedChange => {
      if (c.op === "update") return { ...c, changes: fold(c.changes, c.exprs), exprs: undefined };
      if (c.op === "insert") return { ...c, values: fold(c.values, c.exprs), exprs: undefined };
      return c;
    }),
  };
}

function error(message: string, statementIndex: number | null = null): ErrorPayload {
  return {
    code: "DB_ERROR",
    message,
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex,
    data: null,
  };
}

/** Shown in the Preview SQL sheet of a MySQL replacement (as the backend words it). */
const MYSQL_REPLACE_WARNING =
  "MySQL cannot replace a routine in one step: the old version is dropped, then the new one " +
  "created. If the new definition fails, the previous version is created again from the " +
  "definition read just before.";

const NAME = '(?:"[^"]+"|`[^`]+`|\\w+)';
const SKIP = "(?:\\s|--[^\\n]*\\n|\\/\\*[\\s\\S]*?\\*\\/)*";
const LEADING_DROP = new RegExp(
  `^${SKIP}drop\\s+(function|procedure)\\s+if\\s+exists\\s+(?:(${NAME})\\s*\\.\\s*)?(${NAME})\\s*(?:\\(([^)]*)\\))?\\s*;`,
  "i",
);
const unquote = (s: string) => s.replace(/^(["`])([\s\S]*)\1$/, "$2");

/**
 * A `DROP … IF EXISTS name;` in front of a definition (what opening a MySQL routine puts in its
 * tab) and the text after it: the exact shape only, like the backend (PostgreSQL needs the
 * argument list, MySQL takes none).
 */
function leadingDrop(driver: string, text: string): { routine: RoutineRef; rest: string } | null {
  const m = LEADING_DROP.exec(text);
  if (!m || (driver === "postgres") !== (m[4] !== undefined)) return null;
  const name = unquote(m[3]!);
  return {
    routine: {
      schema: m[2] ? unquote(m[2]) : null,
      name: driver === "postgres" && !/^["`]/.test(m[3]!) ? name.toLowerCase() : name,
      kind: m[1]!.toLowerCase() === "procedure" ? "procedure" : "function",
      identityArguments: (m[4] ?? "").trim().replace(/\s+/g, " "),
    },
    rest: text.slice(m[0].length).trim(),
  };
}

/** Whether two references name the same routine (the backend's `same_routine`). */
function sameRoutine(driver: string, a: RoutineRef, b: RoutineRef): boolean {
  const name =
    driver === "mysql" ? a.name.toLowerCase() === b.name.toLowerCase() : a.name === b.name;
  const norm = (x: string) => x.trim().replace(/\s+/g, " ").toLowerCase();
  return (
    name &&
    (a.kind === "procedure") === (b.kind === "procedure") &&
    (a.schema === null || b.schema === null || a.schema === b.schema) &&
    (driver !== "postgres" || norm(a.identityArguments) === norm(b.identityArguments))
  );
}

/**
 * MySQL creates an unqualified name in the connection's current database: a routine that replaces
 * one gets that one's schema in front of its name (a name that has a schema is left alone).
 */
function qualifiedRoutine(sql: string, schema: string): string {
  const head = new RegExp(
    `^(${SKIP}create\\s+(?:or\\s+replace\\s+)?(?:definer\\s*=\\s*\\S+\\s+)?(?:function|procedure)\\s+(?:if\\s+not\\s+exists\\s+)?)(${NAME})(\\s*\\()`,
    "i",
  );
  return sql.replace(head, (_all, before: string, name: string, open: string) => {
    return `${before}\`${schema.replaceAll("`", "``")}\`.${name}${open}`;
  });
}

/**
 * What saving a definition runs, like `sqlgen::definition::definition_plan`: `DROP … IF EXISTS`
 * of the tab's routine (a `DROP … IF EXISTS` of it typed in front of the text is that same drop,
 * never run twice; one naming another routine is refused), then the text as ONE statement.
 */
function definitionPlan(
  driver: string,
  tab: RoutineRef | null,
  sql: string,
): { statements: string[]; warnings: string[] } {
  const text = sql.trim().replace(/;\s*$/, "").trimEnd();
  if (!text) mockError("INVALID_INPUT", "the definition is empty");
  const lead = leadingDrop(driver, text);
  const definition = lead ? lead.rest.replace(/;\s*$/, "").trimEnd() : text;
  if (!/^(?:\s|--[^\n]*\n|\/\*[\s\S]*?\*\/)*create\b/i.test(definition))
    mockError(
      "INVALID_INPUT",
      "a definition starts with CREATE (CREATE VIEW / FUNCTION / PROCEDURE …); saving drops the old version for you",
    );
  if (tab && lead && !sameRoutine(driver, tab, lead.routine))
    mockError(
      "INVALID_INPUT",
      `the DROP in front of the definition names ${lead.routine.name}, but this tab replaces ${tab.name}: remove that line (saving drops the old version itself)`,
    );
  const drop = tab ?? lead?.routine ?? null;
  const statements: string[] = [];
  if (drop) {
    const word = drop.kind === "procedure" ? "PROCEDURE" : "FUNCTION";
    const q = (s: string) => (driver === "mysql" ? `\`${s}\`` : `"${s}"`);
    const name = drop.schema ? `${q(drop.schema)}.${q(drop.name)}` : q(drop.name);
    statements.push(
      `DROP ${word} IF EXISTS ${name}${driver === "postgres" ? `(${drop.identityArguments})` : ""}`,
    );
  }
  statements.push(
    driver === "mysql" && drop?.schema ? qualifiedRoutine(definition, drop.schema) : definition,
  );
  return { statements, warnings: drop && driver === "mysql" ? [MYSQL_REPLACE_WARNING] : [] };
}

/** The routine a definition's text creates (the header's name and kind), for the next ⌘S. */
function routineOf(driver: string, sql: string, drop: RoutineRef | null): RoutineRef | null {
  const m =
    /^\s*create\s+(?:or\s+replace\s+)?(?:definer\s*=\s*\S+\s+)?(function|procedure)\s+(?:if\s+not\s+exists\s+)?(?:(?:"([^"]+)"|`([^`]+)`|(\w+))\s*\.\s*)?(?:"([^"]+)"|`([^`]+)`|(\w+))\s*\(([^)]*)\)/i.exec(
      sql,
    );
  if (!m) return null;
  const name = m[5] ?? m[6] ?? m[7]!;
  const args = driver === "postgres" ? (m[8] ?? "").replace(/\s+default\s+[^,]+/gi, "").trim() : "";
  return {
    schema: m[2] ?? m[3] ?? m[4] ?? drop?.schema ?? null,
    name: driver === "postgres" && m[5] === undefined ? name.toLowerCase() : name,
    kind: m[1]!.toLowerCase() === "procedure" ? "procedure" : "function",
    identityArguments: args,
  };
}

async function driverOf(connectionId: unknown): Promise<string> {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  return all.find((c) => c.connectionId === connectionId)?.driver ?? "postgres";
}

async function planOne(
  connectionId: unknown,
  driver: string,
  g: WorkspaceGroup,
): Promise<{ statements: string[]; warnings: string[]; transactional: boolean }> {
  switch (g.kind) {
    case "rows":
      return {
        statements: await mockInvoke<string[]>("preview_changes", {
          connectionId,
          req: folded(g.request, "sql", driver),
        }),
        warnings: [],
        transactional: true,
      };
    case "ddl":
    case "object": {
      const plan =
        g.kind === "ddl"
          ? await mockInvoke<DdlPlan>("preview_ddl", { connectionId, table: g.table, ops: g.ops })
          : await mockInvoke<DdlPlan>("preview_object_op", { connectionId, op: g.op });
      return plan;
    }
    case "definition":
      return { ...definitionPlan(driver, g.drop, g.sql), transactional: driver !== "mysql" };
  }
}

const rank = (g: WorkspaceGroup) => (g.kind === "rows" ? 0 : g.kind === "object" ? 2 : 1);

/** `is_atomic`: one transaction runs it all (on MySQL only while row edits are all there is). */
function isAtomic(
  driver: string,
  groups: readonly WorkspaceGroup[],
  plans: readonly { transactional: boolean }[],
): boolean {
  return (
    plans.every((p) => p.transactional) &&
    (driver !== "mysql" || groups.every((g) => g.kind === "rows"))
  );
}

registerMock("preview_workspace_changes", async ({ connectionId, groups }) => {
  const driver = await driverOf(connectionId);
  const out: GroupPlan[] = [];
  for (const g of groups as WorkspaceGroup[]) {
    try {
      out.push({ ...(await planOne(connectionId, driver, g)), error: null });
    } catch (e) {
      const message = (e as { message?: string }).message ?? String(e);
      out.push({ statements: [], warnings: [], transactional: true, error: error(message) });
    }
  }
  return {
    groups: out,
    atomic: out.every((g) => !g.error) && isAtomic(driver, groups as WorkspaceGroup[], out),
  } satisfies WorkspacePlan;
});

registerMock("apply_workspace_changes", async ({ connectionId, groups: raw }) => {
  const started = performance.now();
  const groups = raw as WorkspaceGroup[];
  const driver = await driverOf(connectionId);
  const results: GroupResult[] = groups.map(() => ({
    status: "notRun",
    sql: [],
    executed: [],
    failedIndex: null,
    rows: [],
    routine: null,
    error: null,
  }));
  const plans: Array<{ statements: string[]; transactional: boolean }> = [];
  for (const [i, g] of groups.entries()) {
    try {
      const plan = await planOne(connectionId, driver, g);
      plans.push(plan);
      results[i]!.sql = plan.statements;
    } catch (e) {
      const err = error((e as { message?: string }).message ?? String(e));
      results[i] = { ...results[i]!, status: "failed", error: err };
      return {
        success: false,
        atomic: false,
        groups: results,
        failedGroup: i,
        error: err,
        durationMs: 0,
      } satisfies WorkspaceResult;
    }
  }
  const atomic = isAtomic(driver, groups, plans);
  const order = groups.map((_, i) => i).sort((a, b) => rank(groups[a]!) - rank(groups[b]!));
  const done = (failedGroup: number | null, err: ErrorPayload | null): WorkspaceResult => ({
    success: err === null,
    // A failing row edit ran first and rolled back, and no structure ran: nothing stayed.
    atomic: atomic || (failedGroup !== null && groups[failedGroup]?.kind === "rows"),
    groups: results,
    failedGroup,
    error: err,
    durationMs: Math.round(performance.now() - started),
  });
  for (const i of order) {
    const g = groups[i]!;
    const r = results[i]!;
    if (g.kind === "rows") {
      const res = await mockInvoke<ApplyResult>("apply_changes", {
        connectionId,
        req: folded(g.request, "value", driver),
      });
      r.rows = res.results;
      if (!res.success) {
        const failed = res.results.findIndex((x) => x.error);
        Object.assign(r, {
          status: "failed",
          failedIndex: failed >= 0 ? failed : null,
          error: res.error,
        });
        return done(i, res.error ?? error("Failed"));
      }
    } else if (g.kind === "definition") {
      // Nothing to change in the mock catalog: the statements "ran".
      r.routine = routineOf(driver, plans[i]!.statements.at(-1)!, g.drop);
    } else {
      const res =
        g.kind === "ddl"
          ? await mockInvoke<DdlResult>("apply_ddl", { connectionId, table: g.table, ops: g.ops })
          : await mockInvoke<DdlResult>("apply_object_op", { connectionId, op: g.op });
      if (res.error) {
        Object.assign(r, {
          status: "failed",
          executed: res.executed,
          failedIndex: res.failedIndex,
          error: res.error,
        });
        return done(i, res.error);
      }
    }
    r.status = "applied";
    r.executed = r.sql;
  }
  return done(null, null);
});
