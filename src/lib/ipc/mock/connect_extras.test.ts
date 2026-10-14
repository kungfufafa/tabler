import { describe, expect, it } from "vitest";
import { profile } from "./fixtures";
import { mockSocketCheck } from "./connect_extras";

const ssh = {
  host: "bastion",
  port: 22,
  user: "deploy",
  auth: "agent",
  keyPath: null,
  localPort: null,
  keepaliveSecs: null,
} as const;

function mysql(options: { socketPath?: string | null; ssh?: boolean }) {
  const base = profile({ id: "p-sock", name: "Sock", driver: "mysql" });
  return {
    ...base,
    ssh: options.ssh ? ssh : null,
    options: { ...base.options, useSocket: true, socketPath: options.socketPath ?? null },
  };
}

describe("mock MySQL Use socket check", () => {
  it("accepts a socket over SSH: the server's own path, full or detected", () => {
    expect(() => mockSocketCheck(mysql({ ssh: true }))).not.toThrow();
    expect(() =>
      mockSocketCheck(mysql({ ssh: true, socketPath: "/run/mysqld/m.sock" })),
    ).not.toThrow();
  });

  it("wants a full path over SSH, and reports a missing socket", () => {
    expect(() => mockSocketCheck(mysql({ ssh: true, socketPath: "~/m.sock" }))).toThrow(
      /full path/,
    );
    expect(() => mockSocketCheck(mysql({ ssh: true, socketPath: "/srv/missing.sock" }))).toThrow(
      /socket not found/,
    );
    // Locally any path goes (it is checked to exist).
    expect(() => mockSocketCheck(mysql({ socketPath: "mysql.sock" }))).not.toThrow();
  });
});
