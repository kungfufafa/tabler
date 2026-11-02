import { describe, expect, it } from "vitest";
import { redact } from "./redact";

// The mock's copy of the backend's Vertica redaction (`sqlgen/dialect/vertica.rs` knobs).

describe("the Vertica redaction", () => {
  it("masks the literal after a password keyword and a function's secret argument", () => {
    expect(redact("ALTER USER u IDENTIFIED BY 'p''w'")).toBe(
      "ALTER USER u IDENTIFIED BY '********'",
    );
    expect(redact("ALTER SESSION SET AWSAuth = 'id:secret'")).toBe(
      "ALTER SESSION SET AWSAuth = '********'",
    );
    expect(redact("SELECT AWS_SET_CONFIG('aws_secret_access_key', 'k')")).toBe(
      "SELECT AWS_SET_CONFIG('aws_secret_access_key', '********')",
    );
    expect(redact("SELECT LDAP_LINK_DRYRUN_CONNECT('ldap://h', 'cn=a', 'pw', 1)")).toBe(
      "SELECT LDAP_LINK_DRYRUN_CONNECT('ldap://h', 'cn=a', '********', 1)",
    );
  });

  it("masks the longest literal any reading finds, and leaves other text alone", () => {
    expect(redact(String.raw`CREATE USER u IDENTIFIED BY 'x\'y' -- c`)).toBe(
      "CREATE USER u IDENTIFIED BY '********' -- c",
    );
    expect(redact("SET SEARCH_PATH TO store")).toBe("SET SEARCH_PATH TO store");
    expect(redact("SELECT 'password'")).toBe("SELECT 'password'");
  });
  it("masks lookalike spellings, quoted function names and `U&'…'` as the backend does", () => {
    for (const [sql, masked] of [
      [
        "SELECT LDAP_LIN\u212A_DRYRUN_CONNECT(u, d, 'pw')",
        "SELECT LDAP_LIN\u212A_DRYRUN_CONNECT(u, d, '********')",
      ],
      [
        `SELECT "AWS_SET_CONFIG"('aws_secret_access_key', 'k')`,
        `SELECT "AWS_SET_CONFIG"('aws_secret_access_key', '********')`,
      ],
      [
        `SELECT s."ldap_link_dryrun_connect"(u, d, 'pw')`,
        `SELECT s."ldap_link_dryrun_connect"(u, d, '********')`,
      ],
      [String.raw`ALTER USER u PASSWORD U&'p\0077'`, "ALTER USER u PASSWORD U&'********'"],
      [
        String.raw`ALTER USER u IDENTIFIED BY U&'p\0077'`,
        "ALTER USER u IDENTIFIED BY U&'********'",
      ],
      ["ALTER USER u PA\u017F\u017FWORD 'pw'", "ALTER USER u PA\u017F\u017FWORD '********'"],
    ])
      expect(redact(sql!), sql).toBe(masked);
    expect(redact(`SELECT "other"(u, d, 'pw')`)).toBe(`SELECT "other"(u, d, 'pw')`);
  });
});
