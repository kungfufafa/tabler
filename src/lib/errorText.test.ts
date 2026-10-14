import { describe, expect, it } from "vitest";
import { IpcError } from "@/lib/ipc";
import { errorWithHint } from "./errorText";

describe("errorWithHint", () => {
  it("adds the hint to the message", () => {
    const timeout = new IpcError({
      code: "QUERY_TIMEOUT",
      message: "Query timed out after 300 s",
      hint: "The statement was cancelled. The limit is Settings ▸ Connections ▸ Query time out (0 = no limit).",
    });
    expect(errorWithHint(timeout)).toBe(
      "Query timed out after 300 s The statement was cancelled. The limit is Settings ▸ Connections ▸ Query time out (0 = no limit).",
    );
  });

  it("is just the message without a hint, and reads plain errors and payloads", () => {
    expect(errorWithHint(new IpcError({ code: "NOT_FOUND", message: "table nope" }))).toBe(
      "table nope",
    );
    expect(errorWithHint(new Error("boom"))).toBe("boom");
    expect(errorWithHint({ code: "DB_ERROR", message: "bad", hint: " Try again. " })).toBe(
      "bad Try again.",
    );
  });
});
