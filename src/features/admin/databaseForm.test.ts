import { describe, expect, it } from "vitest";
import type { DatabaseOptions } from "@/lib/ipc";
import {
  autoTemplate,
  collationsFor,
  createDatabaseOp,
  currentDatabaseRefusal,
  defaultCollationOf,
  emptyCreateForm,
  nameError,
} from "./databaseForm";

const pg: DatabaseOptions = {
  encodings: ["UTF8", "LATIN1"].map((name) => ({
    name,
    description: null,
    defaultCollation: null,
  })),
  collations: [
    { name: "C", encoding: null },
    { name: "en_US.UTF-8", encoding: "UTF8" },
    { name: "de_DE.UTF-8", encoding: "UTF8" },
    { name: "en_US.ISO8859-1", encoding: "LATIN1" },
  ],
  templates: ["template0", "template1"],
  defaultEncoding: "UTF8",
  defaultCollation: "en_US.UTF-8",
};

const my: DatabaseOptions = {
  encodings: [
    { name: "latin1", description: "cp1252", defaultCollation: "latin1_swedish_ci" },
    { name: "utf8mb4", description: "UTF-8", defaultCollation: "utf8mb4_0900_ai_ci" },
  ],
  collations: [
    { name: "latin1_bin", encoding: "latin1" },
    { name: "utf8mb4_bin", encoding: "utf8mb4" },
    { name: "utf8mb4_0900_ai_ci", encoding: "utf8mb4" },
  ],
  templates: [],
  defaultEncoding: "utf8mb4",
  defaultCollation: null,
};

describe("create database form", () => {
  it("starts on the server's default encoding", () => {
    expect(emptyCreateForm(pg)).toEqual({
      name: "",
      encoding: "UTF8",
      collation: "",
      template: null,
    });
    expect(emptyCreateForm(undefined).encoding).toBe("");
  });

  it("offers the collations of the chosen encoding (and PG's encoding-free ones)", () => {
    expect(collationsFor(pg, "UTF8").map((c) => c.name)).toEqual([
      "C",
      "en_US.UTF-8",
      "de_DE.UTF-8",
    ]);
    expect(collationsFor(pg, "latin1").map((c) => c.name)).toEqual(["C", "en_US.ISO8859-1"]);
    expect(collationsFor(my, "utf8mb4").map((c) => c.name)).toEqual([
      "utf8mb4_bin",
      "utf8mb4_0900_ai_ci",
    ]);
    expect(defaultCollationOf(my, "latin1")).toBe("latin1_swedish_ci");
    expect(collationsFor(undefined, "UTF8")).toEqual([]);
  });

  it("copies from template0 only when template1 cannot give the encoding or collation", () => {
    expect(autoTemplate("postgres", pg, { encoding: "UTF8", collation: "" })).toBe("");
    expect(autoTemplate("postgres", pg, { encoding: "UTF8", collation: "en_US.UTF-8" })).toBe("");
    expect(autoTemplate("postgres", pg, { encoding: "LATIN1", collation: "" })).toBe("template0");
    expect(autoTemplate("postgres", pg, { encoding: "UTF8", collation: "de_DE.UTF-8" })).toBe(
      "template0",
    );
    expect(autoTemplate("mysql", my, { encoding: "latin1", collation: "" })).toBe("");
    expect(
      autoTemplate(
        "postgres",
        { ...pg, templates: ["template1"] },
        { encoding: "LATIN1", collation: "" },
      ),
    ).toBe("");
  });

  it("builds the op, blank choices meaning the server default", () => {
    expect(
      createDatabaseOp("postgres", pg, {
        name: " shop ",
        encoding: "LATIN1",
        collation: "",
        template: null,
      }),
    ).toEqual({
      op: "createDatabase",
      name: "shop",
      encoding: "LATIN1",
      collation: null,
      template: "template0",
    });
    // An explicit choice wins over the automatic one, "" included.
    expect(
      createDatabaseOp("postgres", pg, {
        name: "a",
        encoding: "LATIN1",
        collation: "",
        template: "",
      }).template,
    ).toBeNull();
    expect(
      createDatabaseOp("mysql", my, {
        name: "a",
        encoding: "utf8mb4",
        collation: "utf8mb4_bin",
        template: "template0",
      }),
    ).toEqual({
      op: "createDatabase",
      name: "a",
      encoding: "utf8mb4",
      collation: "utf8mb4_bin",
      template: null,
    });
  });

  it("checks names", () => {
    expect(nameError("  ", [], "database")).toBe("Enter a database name");
    expect(nameError("shop", ["shop"], "database")).toBe("A database named shop already exists");
    expect(nameError("sales", ["shop"], "schema")).toBeNull();
    expect(currentDatabaseRefusal("app", "drop")).toContain("Switch to another database");
  });
});
