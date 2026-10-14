import { describe, expect, it } from "vitest";
import type { DumpBinary, ParsedVersion } from "@/lib/ipc";
import { installHelp } from "./install";

const pg = (text: string): ParsedVersion => {
  const [major = 0, minor = 0] = text.split(".").map(Number);
  return { flavor: "postgres", text, major, minor };
};
const bin = (tool: DumpBinary["tool"], version: ParsedVersion): DumpBinary => ({
  tool,
  path: `/opt/homebrew/bin/${tool}`,
  version,
  versionOutput: "",
});

describe("installHelp", () => {
  it("offers libpq and the server's version when no pg_dump can serve the server", () => {
    const none = installHelp("pg_dump", [], pg("17.6"));
    expect(none?.reason).toContain("No pg_dump found");
    expect(none?.formulas.map((f) => f.formula)).toEqual(["libpq", "postgresql@17"]);
    expect(none?.download.url).toContain("postgresql.org");
    const old = installHelp("pg_dump", [bin("pg_dump", pg("15.8"))], pg("17.6"));
    expect(old?.reason).toContain("older than the server (PostgreSQL 17.6)");
    // Old servers: no matching Homebrew formula, libpq still dumps them.
    expect(installHelp("pg_dump", [], pg("9.6"))?.formulas.map((f) => f.formula)).toEqual([
      "libpq",
    ]);
  });

  it("is not shown while a binary can serve the server", () => {
    expect(installHelp("pg_dump", [bin("pg_dump", pg("18.2"))], pg("17.6"))).toBeNull();
    expect(installHelp("pg_dump", [bin("pg_dump", pg("17.1"))], pg("17.6"))).toBeNull();
    expect(installHelp("psql", [bin("psql", pg("16.0"))], null)).toBeNull();
    // Another tool does not count.
    expect(installHelp("pg_restore", [bin("pg_dump", pg("18.2"))], pg("17.6"))).not.toBeNull();
  });

  it("offers the MySQL client, or MariaDB for a MariaDB server", () => {
    const mysql8: ParsedVersion = { flavor: "mysql", text: "8.0.36", major: 8, minor: 0 };
    expect(installHelp("mysqldump", [], mysql8)?.formulas.map((f) => f.formula)).toEqual([
      "mysql-client",
      "mysql-client@8.4",
    ]);
    const maria: ParsedVersion = { flavor: "mariadb", text: "11.8.9", major: 11, minor: 8 };
    const help = installHelp("mysql", [], maria);
    expect(help?.formulas.map((f) => f.formula)).toEqual(["mariadb"]);
    expect(help?.download.url).toContain("mariadb.org");
  });
});
