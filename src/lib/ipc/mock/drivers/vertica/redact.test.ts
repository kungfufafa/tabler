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
});
