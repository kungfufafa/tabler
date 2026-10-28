import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  ipc,
  type BrowsePage,
  type BrowseRequest,
  type ConnectionProfile,
  type ProfileInput,
} from "@/lib/ipc";
import type { InvokeArgs } from "../transport";
import { profile } from "./fixtures";
import { mockHandler, registerMock } from "./transport";

const input = (p: ConnectionProfile): ProfileInput => ({
  profile: p,
  password: null,
  sshPassword: null,
  sshPassphrase: null,
  clearPassword: false,
});

/**
 * Runs `body` with `cmd`'s mock replaced by one that records the arguments it is sent and answers
 * `reply(answer)`, `answer` being the mock's own (null when it refuses); then puts the mock back.
 */
async function recording<T>(
  cmd: string,
  reply: (answer: unknown) => unknown,
  body: () => Promise<T>,
): Promise<{ sent: InvokeArgs[]; result: T }> {
  const own = mockHandler(cmd)!;
  const sent: InvokeArgs[] = [];
  registerMock(cmd, async (args) => {
    sent.push(args);
    let answer: unknown = null;
    try {
      answer = await own(args);
    } catch {
      // The core drivers have no calls: only the arguments matter here.
    }
    return reply(answer);
  });
  try {
    return { sent, result: await body() };
  } finally {
    registerMock(cmd, own);
  }
}

describe("mock driver calls", () => {
  let c = "";
  beforeAll(async () => {
    c = (await ipc.connections.connect("p-local-pg")).connectionId;
  });
  afterAll(async () => {
    await ipc.connections.disconnect(c);
  });

  it("refuses a core connection's calls as unknown, as the backend does", async () => {
    await expect(ipc.sessionCall(c, "stats")).rejects.toMatchObject({
      code: "UNSUPPORTED",
      message: "unsupported: PostgreSQL: unknown call “stats”",
    });
    await expect(ipc.sessionCall("gone", "stats")).rejects.toMatchObject({
      code: "NOT_CONNECTED",
    });
  });

  it("refuses the form's calls of a core driver, and of a database this build lacks", async () => {
    const mysql = profile({ id: "p-util-my", name: "My", driver: "mysql" });
    await expect(ipc.driverUtil(input(mysql), "probe")).rejects.toMatchObject({
      message: "unsupported: MySQL: unknown call “probe”",
    });
    const redis = profile({ id: "p-util-redis", name: "Cache", driver: "redis" });
    await expect(ipc.driverUtil(input(redis), "probe")).rejects.toMatchObject({
      message: "unsupported: Redis is not built into this Tabler",
    });
  });

  // The argument names are the Rust commands' parameters (`session_call(connection_id, method,
  // args)`, `driver_util(input, method, args)`); a call without arguments sends JSON null.
  it("sends a call's method and arguments under the commands' parameter names", async () => {
    const calls = await recording(
      "session_call",
      () => ["user:1"],
      async () => [
        await ipc.sessionCall(c, "keys", { match: "user:*" }),
        await ipc.sessionCall(c, "info"),
      ],
    );
    expect(calls.result).toEqual([["user:1"], ["user:1"]]);
    expect(calls.sent).toEqual([
      { connectionId: c, method: "keys", args: { match: "user:*" } },
      { connectionId: c, method: "info", args: null },
    ]);

    const mysql = input(profile({ id: "p-util-my", name: "My", driver: "mysql" }));
    const utils = await recording(
      "driver_util",
      () => ["analytics"],
      () => ipc.driverUtil(mysql, "databases"),
    );
    expect(utils.result).toEqual(["analytics"]);
    expect(utils.sent).toEqual([{ input: mysql, method: "databases", args: null }]);
  });

  it("hands a browse cursor to the backend and its next one back", async () => {
    const req: BrowseRequest = {
      table: { schema: "public", name: "users" },
      columns: null,
      filter: null,
      sort: [],
      limit: 5,
      offset: 0,
      cursor: "page-2",
    };
    const { sent, result } = await recording(
      "browse_table",
      (answer) => ({ ...(answer as BrowsePage), nextCursor: "page-3" }),
      () => ipc.browse.rows(c, req),
    );
    expect(sent.map((a) => (a.req as BrowseRequest).cursor)).toEqual(["page-2"]);
    expect(result.nextCursor).toBe("page-3");
    expect(result.rows).toHaveLength(5);
  });
});
