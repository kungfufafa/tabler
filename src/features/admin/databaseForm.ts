import type { AdminDdl, CollationOption, DatabaseOptions, Driver } from "@/lib/ipc";

/** What the Create Database dialog holds. Empty strings mean "Default" (the server picks). */
export interface CreateDatabaseForm {
  name: string;
  encoding: string;
  collation: string;
  /** PG template; `null` = chosen automatically (see `autoTemplate`). */
  template: string | null;
}

export function emptyCreateForm(options: DatabaseOptions | undefined): CreateDatabaseForm {
  return {
    name: "",
    encoding: options?.defaultEncoding ?? "",
    collation: "",
    template: null,
  };
}

/** Collations usable with `encoding`: MySQL by character set, PG by encoding (or any). */
export function collationsFor(
  options: DatabaseOptions | undefined,
  encoding: string,
): CollationOption[] {
  if (!options) return [];
  const enc = encoding.toLowerCase();
  return options.collations.filter(
    (c) => c.encoding === null || enc === "" || c.encoding.toLowerCase() === enc,
  );
}

/** The collation MySQL picks for `encoding` when none is given. */
export function defaultCollationOf(
  options: DatabaseOptions | undefined,
  encoding: string,
): string | null {
  return options?.encodings.find((e) => e.name === encoding)?.defaultCollation ?? null;
}

/**
 * PostgreSQL copies new databases from `template1`, which only allows its own encoding and
 * locale: another encoding or collation needs `template0`. Returns the template to use when the
 * user left it on automatic.
 */
export function autoTemplate(
  driver: Driver,
  options: DatabaseOptions | undefined,
  form: Pick<CreateDatabaseForm, "encoding" | "collation">,
): string {
  if (driver !== "postgres" || !options?.templates.includes("template0")) return "";
  const encoding = form.encoding.toLowerCase();
  const defaultEncoding = (options.defaultEncoding ?? "").toLowerCase();
  const otherEncoding = encoding !== "" && defaultEncoding !== "" && encoding !== defaultEncoding;
  const otherCollation =
    form.collation !== "" && form.collation !== (options.defaultCollation ?? "");
  return otherEncoding || otherCollation ? "template0" : "";
}

/** Template the statement uses: the user's choice, else the automatic one. */
export function effectiveTemplate(
  driver: Driver,
  options: DatabaseOptions | undefined,
  form: CreateDatabaseForm,
): string {
  return form.template ?? autoTemplate(driver, options, form);
}

/** Why `name` cannot be a new database / schema, or null. */
export function nameError(
  name: string,
  existing: readonly string[],
  what: "database" | "schema",
): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return `Enter a ${what} name`;
  if (existing.some((e) => e === trimmed)) return `A ${what} named ${trimmed} already exists`;
  return null;
}

export function createDatabaseOp(
  driver: Driver,
  options: DatabaseOptions | undefined,
  form: CreateDatabaseForm,
): Extract<AdminDdl, { op: "createDatabase" }> {
  const blank = (s: string) => (s.trim() === "" ? null : s.trim());
  return {
    op: "createDatabase",
    name: form.name.trim(),
    encoding: blank(form.encoding),
    collation: blank(form.collation),
    template: driver === "postgres" ? blank(effectiveTemplate(driver, options, form)) : null,
  };
}

/** Message of the refusal to rename / drop the database the connection is using. */
export function currentDatabaseRefusal(database: string, action: "rename" | "drop"): string {
  return `${database} is the database this connection is using. Switch to another database (⌘K) to ${action} it.`;
}
