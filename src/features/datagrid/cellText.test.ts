import { describe, expect, it } from "vitest";
import { exprOf } from "@/features/staging/expr";
import { bytesHex, isNowText, parseCellText, parseHexBytes } from "./cellText";

describe("NOW() typed in a date / time cell", () => {
  it("recognises NOW() and CURRENT_TIMESTAMP spellings", () => {
    for (const t of [
      "NOW()",
      "now( )",
      "CURRENT_TIMESTAMP",
      "current_timestamp(3)",
      "CURRENT_DATE",
      "NOW(6)",
      "localtimestamp( 0 )",
    ])
      expect(isNowText(t)).toBe(true);
    expect(isNowText("NOW")).toBe(false);
    expect(isNowText("NOW(7)")).toBe(false);
    expect(isNowText("now() + interval '1 day'")).toBe(false);
    expect(isNowText("2024-01-01")).toBe(false);
  });
  it("stages the server's NOW() (an expression, not this computer's clock)", () => {
    const r = parseCellText({ kind: "timestamp", nullable: true }, "now()");
    expect(r.ok && r.value).toEqual({ t: "other", v: { ty: "\u0000expr", text: "NOW()" } });
    const d = parseCellText({ kind: "date", nullable: true }, "CURRENT_TIMESTAMP");
    expect(d.ok && d.value.t === "other" && d.value.v.text).toBe("CURRENT_TIMESTAMP");
    // A typed precision is kept (a DATETIME(6) cell typed CURRENT_TIMESTAMP(6) stores microseconds).
    for (const [typed, shown, expr] of [
      ["CURRENT_TIMESTAMP(6)", "CURRENT_TIMESTAMP(6)", "currentTimestamp(6)"],
      ["now(6)", "NOW(6)", "now(6)"],
      ["current_time(3)", "CURRENT_TIME(3)", "currentTime(3)"],
      ["LOCALTIMESTAMP(1)", "LOCALTIMESTAMP(1)", "localTimestamp(1)"],
    ] as const) {
      const p = parseCellText({ kind: "timestamp", nullable: true }, typed);
      expect(p.ok && p.value, typed).toEqual({ t: "other", v: { ty: "\u0000expr", text: shown } });
      expect(p.ok && exprOf(p.value), typed).toBe(expr);
    }
    // More than 6 digits is no expression: plain text, which the server rejects.
    expect(parseCellText({ kind: "timestamp", nullable: true }, "NOW(7)")).toEqual({
      ok: true,
      value: { t: "timestamp", v: "NOW(7)" },
    });
    // Elsewhere it is just text.
    expect(parseCellText({ kind: "text", nullable: true }, "NOW()")).toEqual({
      ok: true,
      value: { t: "text", v: "NOW()" },
    });
  });
});

describe("bytes as hex", () => {
  it("round-trips hex and base64", () => {
    expect(bytesHex(btoa("Hi!"))).toBe("486921");
    expect(bytesHex(btoa("0123456789abcdefg"), true)).toBe(
      "30 31 32 33 34 35 36 37 38 39 61 62 63 64 65 66\n67",
    );
    expect(parseHexBytes("\\x48 69\n21")).toEqual({
      ok: true,
      value: { t: "bytes", v: { base64: btoa("Hi!"), len: 3, truncated: false } },
    });
    expect(parseHexBytes("0xFF")).toMatchObject({ ok: true, value: { v: { len: 1 } } });
    expect(parseHexBytes("abc").ok).toBe(false);
    expect(parseHexBytes("zz").ok).toBe(false);
  });
  it("an emptied nullable blob is NULL", () => {
    expect(parseCellText({ kind: "bytes", nullable: true }, " ")).toEqual({
      ok: true,
      value: { t: "null" },
    });
    expect(parseCellText({ kind: "bytes", nullable: false }, "")).toMatchObject({
      ok: true,
      value: { v: { len: 0 } },
    });
  });
});
