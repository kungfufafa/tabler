import { describe, expect, it } from "vitest";
import { emptyBecomesNull, nowValue } from "./setValue";

describe("nowValue", () => {
  const at = new Date(2026, 9, 2, 7, 5, 9);
  it("formats the moment for each date / time kind", () => {
    expect(nowValue("date", at)).toEqual({ t: "date", v: "2026-10-02" });
    expect(nowValue("time", at)).toEqual({ t: "time", v: "07:05:09" });
    expect(nowValue("timestamp", at)).toEqual({ t: "timestamp", v: "2026-10-02 07:05:09" });
    const tz = nowValue("timestampTz", at);
    expect(tz?.t).toBe("timestampTz");
    expect(tz?.t === "timestampTz" && tz.v).toMatch(/^2026-10-02 07:05:09[+-]\d{2}:\d{2}$/);
  });
  it("gives timestampTz the local UTC offset, so the server stores the same instant", () => {
    const moment = new Date(2026, 0, 15, 23, 30, 0);
    const tz = nowValue("timestampTz", moment);
    if (tz?.t !== "timestampTz") throw new Error("not a timestampTz");
    // `YYYY-MM-DD HH:MM:SS±HH:MM` read back as ISO is the moment it was taken.
    expect(new Date(tz.v.replace(" ", "T")).getTime()).toBe(moment.getTime());
    const offset = -moment.getTimezoneOffset();
    const sign = offset < 0 ? "-" : "+";
    const hh = String(Math.floor(Math.abs(offset) / 60)).padStart(2, "0");
    const mm = String(Math.abs(offset) % 60).padStart(2, "0");
    expect(tz.v.endsWith(`${sign}${hh}:${mm}`)).toBe(true);
  });
  it("is null for other kinds", () => {
    expect(nowValue("text", at)).toBeNull();
    expect(nowValue("int", at)).toBeNull();
  });
});

describe("emptyBecomesNull", () => {
  const col = (kind: Parameters<typeof emptyBecomesNull>[0]["kind"], nullable = true) => ({
    kind,
    nullable,
  });
  it("converts empty numeric, UUID, JSON and array input in nullable columns", () => {
    for (const kind of ["int", "bigInt", "decimal", "float", "uuid", "json", "array"] as const) {
      expect(emptyBecomesNull(col(kind), "", true), kind).toBe(true);
      expect(emptyBecomesNull(col(kind), "  ", true), kind).toBe(true);
    }
  });
  it("leaves text kinds, NOT NULL columns, the text NULL and a disabled preference alone", () => {
    expect(emptyBecomesNull(col("text"), "", true)).toBe(false);
    expect(emptyBecomesNull(col("date"), "", true)).toBe(false);
    expect(emptyBecomesNull(col("int", false), "", true)).toBe(false);
    expect(emptyBecomesNull(col("int"), "NULL", true)).toBe(false);
    expect(emptyBecomesNull(col("int"), "", false)).toBe(false);
  });
});
