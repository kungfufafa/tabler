import type { DiagramRelation, DiagramSchema, DiagramTable } from "@/lib/ipc";

/** A table with `columns` as `name:type` (a `*` prefix marks the primary key, `?` a nullable). */
export function table(name: string, columns: string[], kind: DiagramTable["kind"] = "table") {
  return {
    ref: { schema: "public", name },
    kind,
    columns: columns.map((spec) => {
      const isPk = spec.startsWith("*");
      const nullable = spec.includes("?");
      const [col, type] = spec.replace(/^\*/, "").replace("?", "").split(":");
      return { name: col!, dataType: type ?? "integer", isPk, nullable };
    }),
    rowEstimate: null,
  } satisfies DiagramTable;
}

export function relation(
  from: string,
  fromColumn: string,
  to: string,
  toColumn = "id",
  name = `${from}_${fromColumn}_fkey`,
  toSchema = "public",
): DiagramRelation {
  return {
    name,
    from: { schema: "public", name: from },
    fromColumns: [fromColumn],
    to: { schema: toSchema, name: to },
    toColumns: [toColumn],
    onDelete: "CASCADE",
  };
}

/** A small shop schema: a chain, a fan-in, a self reference, a cycle and unrelated tables. */
export function shopSchema(): DiagramSchema {
  const tables = [
    table("users", ["*id:bigint", "email:text", "name:text", "manager_id?:bigint"]),
    table("orders", ["*id:bigint", "user_id:bigint", "coupon_id?:integer", "total:numeric(12,2)"]),
    table("order_items", [
      "*order_id:bigint",
      "*line_no:integer",
      "product_id:bigint",
      "qty:integer",
    ]),
    table("products", ["*id:bigint", "name:text", "category_id:integer"]),
    table("categories", ["*id:integer", "parent_id?:integer", "name:text"]),
    table("coupons", ["*id:integer", "code:text"]),
    table("a_cycle", ["*id:integer", "b_id:integer"]),
    table("b_cycle", ["*id:integer", "a_id:integer"]),
    table("settings", ["*key:text", "value:jsonb"]),
    table("audit_log", ["at:timestamp", "actor:text", "payload:jsonb"]),
    table("active_users", ["id:bigint", "email:text"], "view"),
  ];
  const relations = [
    relation("orders", "user_id", "users"),
    relation("orders", "coupon_id", "coupons"),
    relation("order_items", "order_id", "orders"),
    relation("order_items", "product_id", "products"),
    relation("products", "category_id", "categories"),
    relation("categories", "parent_id", "categories"),
    relation("users", "manager_id", "users"),
    relation("a_cycle", "b_id", "b_cycle"),
    relation("b_cycle", "a_id", "a_cycle"),
    relation("orders", "user_id", "accounts", "id", "orders_account_fkey", "billing"),
  ];
  return { schema: "public", tables, relations };
}
