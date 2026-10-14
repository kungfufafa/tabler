import {
  CompletionContext,
  pickedCompletion,
  type Completion,
  type CompletionResult,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";
import type { TableMeta } from "@/lib/ipc";
import { languageExtension, type LanguageConfig } from "./cmExtensions";
import { completionNamespace } from "./completion";
import {
  allFieldsSource,
  autoWhitespace,
  functionCompletionSource,
  joinCompletionSource,
} from "./sqlCompletion";
import { builtinFunctions } from "./sqlFunctions";
import {
  findTable,
  identParts,
  quoteIdent,
  resolveQualifier,
  tablesInStatement,
} from "./statementTables";

const TABLES = {
  public: {
    users: ["id", "email", "Created At"],
    orders: ["id", "user_id", "total"],
    order_items: ["order_id", "line", "sku"],
  },
  billing: { invoices: ["id", "order_id"] },
};

function meta(name: string, fks: TableMeta["foreignKeys"]): TableMeta {
  return {
    table: { schema: "public", name },
    kind: "table",
    columns: [],
    primaryKey: [],
    indexes: [],
    foreignKeys: fks,
    triggers: [],
    comment: null,
    rowEstimate: null,
    ddl: null,
    checks: [],
    options: null,
  };
}

const fk = (
  name: string,
  columns: string[],
  table: string,
  refColumns: string[],
  schema: string | null = "public",
) => ({
  name,
  columns,
  refTable: { schema, name: table },
  refColumns,
  onUpdate: "NO ACTION",
  onDelete: "NO ACTION",
  definition: null,
});

const METAS: Record<string, TableMeta> = {
  users: meta("users", []),
  orders: meta("orders", [fk("orders_user_fk", ["user_id"], "users", ["id"])]),
  order_items: meta("order_items", [fk("items_order_fk", ["order_id"], "orders", ["id"])]),
};
const tableMeta = vi.fn(async (t: { name: string }) => METAS[t.name] ?? null);

function stateOf(doc: string, cfg: Partial<LanguageConfig> = {}) {
  const state = EditorState.create({
    doc,
    extensions: languageExtension({
      dialect: "postgres",
      schema: completionNamespace({ defaultSchema: "public", schemas: TABLES }),
      defaultSchema: "public",
      tables: TABLES,
      tableMeta,
      ...cfg,
    }),
  });
  ensureSyntaxTree(state, state.doc.length, 5_000);
  return state;
}

async function run(source: CompletionSource, doc: string, explicit = false) {
  const state = stateOf(doc);
  return (await source(
    new CompletionContext(state, doc.length, explicit),
  )) as CompletionResult | null;
}

/** Every option every source of the language offers at the end of `doc`. */
async function allOptions(doc: string, cfg: Partial<LanguageConfig> = {}, explicit = true) {
  const state = stateOf(doc, cfg);
  const sources = state.languageDataAt<CompletionSource>("autocomplete", doc.length);
  const results = await Promise.all(
    sources.map((s) => s(new CompletionContext(state, doc.length, explicit))),
  );
  return results.flatMap((r) => (r ? r.options : []));
}

describe("statement tables", () => {
  it("finds the tables of FROM / JOIN / UPDATE with their aliases", () => {
    const uses = tablesInStatement(
      `SELECT * FROM public.users u, "Orders" AS o -- users x\n LEFT JOIN order_items i ON i.order_id = o.id WHERE 'from nope'`,
    );
    expect(uses.map((u) => [u.parts, u.alias, u.joined])).toEqual([
      [["public", "users"], "u", false],
      [["Orders"], "o", false],
      [["order_items"], "i", true],
    ]);
    expect(tablesInStatement("UPDATE users SET x = 1")[0]).toMatchObject({
      parts: ["users"],
      alias: null,
    });
    expect(tablesInStatement("select 1 from users where id = 1")[0]!.alias).toBeNull();
  });

  it("resolves qualifiers, names and quoting", () => {
    expect(resolveQualifier("select from users u join orders o", ["o"])).toEqual(["orders"]);
    expect(resolveQualifier("select from public.users", ["users"])).toEqual(["public", "users"]);
    expect(resolveQualifier("select 1", ["users"])).toEqual(["users"]);
    expect(findTable(TABLES, ["USERS"], "public")?.name).toBe("users");
    expect(findTable(TABLES, ["invoices"], "public")?.schema).toBe("billing");
    expect(findTable(TABLES, ["billing", "nope"], "public")).toBeNull();
    expect(identParts('public."My Table"')).toEqual(["public", "My Table"]);
    expect(quoteIdent("users", "postgres")).toBe("users");
    expect(quoteIdent("Created At", "postgres")).toBe('"Created At"');
    expect(quoteIdent("Users", "mysql")).toBe("Users");
    expect(quoteIdent("a`b", "mysql")).toBe("`a``b`");
  });
});

describe("functions", () => {
  it("offers built-ins and routines with brackets, not after a dot", async () => {
    const source = functionCompletionSource({
      dialect: "postgres",
      upperCase: true,
      closeBrackets: true,
      routines: [{ name: "refresh_stats", kind: "procedure" }],
    });
    const r = await run(source, "select coa");
    const labels = r!.options.map((o) => o.label);
    expect(labels).toContain("COALESCE");
    expect(labels).toContain("refresh_stats");
    expect(r!.from).toBe(7);
    expect(await run(source, "select u.co")).toBeNull();
    expect(await run(source, "select 'coa")).toBeNull();
    expect(builtinFunctions("mysql")).toContain("group_concat");
    expect(builtinFunctions("sqlite")).toContain("strftime");
  });

  it("inserts name() with the caret inside", () => {
    const view = new EditorView({ doc: "select cou", parent: document.body });
    const source = functionCompletionSource({
      dialect: "postgres",
      upperCase: true,
      closeBrackets: true,
    });
    const r = source(new CompletionContext(view.state, 10, false)) as CompletionResult;
    const count = r.options.find((o) => o.label === "COUNT")!;
    (count.apply as (v: EditorView, c: Completion, f: number, t: number) => void)(
      view,
      count,
      r.from,
      10,
    );
    expect(view.state.doc.toString()).toBe("select COUNT()");
    expect(view.state.selection.main.head).toBe(13);
    view.destroy();
  });

  it("keeps keywords that are not functions, and drops COUNT as a keyword", async () => {
    const options = await allOptions("select co");
    expect(options.filter((o) => o.label === "COUNT").map((o) => o.type)).toEqual(["function"]);
    const left = await allOptions("select * from users le");
    expect(left.some((o) => o.label === "LEFT" && o.type === "keyword")).toBe(true);
  });
});

describe("add all table fields", () => {
  it("expands users.* (or an alias) to every column", async () => {
    const source = allFieldsSource({
      dialect: "postgres",
      tables: TABLES,
      defaultSchema: "public",
    });
    let r = await run(source, "select users.");
    expect(r!.options[0]).toMatchObject({
      label: "users.*",
      apply: 'users.id, users.email, users."Created At"',
    });
    r = await run(source, "select o.* from orders o");
    // The cursor is at the end: the qualifier before it is not `o.` there.
    expect(r).toBeNull();
    const state = stateOf("select o.* from orders o");
    r = source(new CompletionContext(state, 10, false)) as CompletionResult;
    expect(r.from).toBe(7);
    expect(r.options[0]!.apply).toBe("o.id, o.user_id, o.total");
    expect(await run(source, "select nope.")).toBeNull();
  });
});

describe("JOIN … ON", () => {
  it("suggests ON conditions from foreign keys in either direction", async () => {
    const source = joinCompletionSource({
      dialect: "postgres",
      tables: TABLES,
      defaultSchema: "public",
      upperCase: true,
      tableMeta,
    });
    let r = await run(source, "select * from users u join orders o ");
    expect(r!.options.map((o) => o.label)).toEqual(["ON o.user_id = u.id"]);
    r = await run(source, "select * from users u join orders o on ");
    expect(r!.options.map((o) => o.label)).toEqual(["o.user_id = u.id"]);
    r = await run(source, "select * from orders join order_items it O");
    expect(r!.options.map((o) => o.label)).toEqual(["ON it.order_id = orders.id"]);
    expect(r!.from).toBe("select * from orders join order_items it ".length);
    // No foreign key between them, or not after a JOIN.
    expect(await run(source, "select * from users join order_items i ")).toBeNull();
    expect(await run(source, "select * from users u where ")).toBeNull();
    expect(await run(source, "select * from users u join orders o using ")).toBeNull();
  });
});

describe("schema prefix, toggles and whitespace", () => {
  it("prefixes tables with their schema and lists other schemas' tables", async () => {
    const options = await allOptions("select * from us", { prefixSchemas: true }, false);
    expect(options.find((o) => o.label === "users")).toMatchObject({ apply: "public.users" });
    expect(options.find((o) => o.label === "invoices")).toMatchObject({
      apply: "billing.invoices",
    });
    const plain = await allOptions("select * from us", {}, false);
    expect(plain.find((o) => o.label === "users")?.apply).toBeUndefined();
    expect(plain.some((o) => o.label === "invoices")).toBe(false);
  });

  it("leaves tables and keywords out when their options are off", async () => {
    const noTables = await allOptions("select * from us", { completeTables: false });
    expect(noTables.some((o) => o.label === "users")).toBe(false);
    const noKeywords = await allOptions("sel", { completeKeywords: false });
    expect(noKeywords.some((o) => o.label === "SELECT")).toBe(false);
    expect((await allOptions("sel")).some((o) => o.label === "SELECT")).toBe(true);
  });

  it("adds a space after an accepted suggestion, unless punctuation follows", () => {
    const state = EditorState.create({
      doc: "select us",
      extensions: autoWhitespace(new Set(["public"])),
    });
    const pick = (label: string, type: string, at = 9) =>
      state.update({
        changes: { from: 7, to: at, insert: label },
        selection: { anchor: 7 + label.length },
        annotations: pickedCompletion.of({ label, type }),
      }).state;
    expect(pick("users", "type").doc.toString()).toBe("select users ");
    expect(pick("users", "type").selection.main.head).toBe(13);
    expect(pick("public", "type").doc.toString()).toBe("select public");
    expect(pick("COUNT()", "function").doc.toString()).toBe("select COUNT()");
    const before = EditorState.create({
      doc: "select us, x",
      extensions: autoWhitespace(new Set()),
    });
    const t = before.update({
      changes: { from: 7, to: 9, insert: "users" },
      selection: { anchor: 12 },
      annotations: pickedCompletion.of({ label: "users", type: "type" }),
    });
    expect(t.state.doc.toString()).toBe("select users, x");
  });
});
