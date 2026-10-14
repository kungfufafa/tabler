import { describe, expect, it } from "vitest";
import {
  bytesHex,
  cellDisplay,
  decodeBytesText,
  displayValue,
  EMPTY_PLACEHOLDER,
  hasLineBreak,
  mergeDatePart,
  pgArrayLiteral,
  splitDatePart,
  valuesEqual,
  valueText,
} from "./values";

const b64 = (bytes: number[]) => btoa(String.fromCharCode(...bytes));
const blob = (bytes: number[], truncated = false) =>
  ({
    t: "bytes",
    v: { base64: b64(bytes), len: bytes.length, truncated },
  }) as const;
const utf8 = (text: string) => Array.from(new TextEncoder().encode(text));

describe("values", () => {
  it("renders every kind on one line", () => {
    expect(displayValue({ t: "null" })).toEqual({ text: "NULL", cls: "null" });
    expect(displayValue({ t: "bigInt", v: "9007199254740993" }).text).toBe("9007199254740993");
    expect(
      displayValue({ t: "bytes", v: { base64: "AA==", len: 2048, truncated: false } }).text,
    ).toBe("<blob 2.0 KB>");
    expect(displayValue({ t: "json", v: '{\n  "a": 1\n}' }).text).toBe('{ "a": 1 }');
    expect(displayValue({ t: "text", v: "a\nb" }).text).toBe("a↵b");
    expect(displayValue({ t: "array", v: [{ t: "text", v: "x y" }, { t: "null" }] }).text).toBe(
      '{"x y",NULL}',
    );
  });
  it("shows an empty value as an EMPTY placeholder in grid cells only", () => {
    expect(cellDisplay({ t: "text", v: "" })).toEqual({
      text: EMPTY_PLACEHOLDER,
      cls: "empty",
      placeholder: true,
    });
    expect(cellDisplay({ t: "enum", v: "" }).cls).toBe("empty");
    expect(cellDisplay({ t: "other", v: { ty: "bit", text: "" } }).cls).toBe("empty");
    expect(cellDisplay({ t: "null" })).toEqual({ text: "NULL", cls: "null", placeholder: true });
    expect(cellDisplay({ t: "default" })).toEqual({
      text: "DEFAULT",
      cls: "default",
      placeholder: true,
    });
    // Whitespace is a value, not an empty string.
    expect(cellDisplay({ t: "text", v: " " })).toEqual({
      text: " ",
      cls: "text",
      placeholder: false,
    });
    expect(cellDisplay({ t: "int", v: 0 })).toEqual({ text: "0", cls: "num", placeholder: false });
    // The underlying helpers keep the empty string.
    expect(displayValue({ t: "text", v: "" })).toEqual({ text: "", cls: "text" });
    expect(valueText({ t: "text", v: "" })).toBe("");
  });
  it("compares structurally", () => {
    expect(valuesEqual({ t: "int", v: 1 }, { t: "int", v: 1 })).toBe(true);
    expect(valuesEqual({ t: "int", v: 1 }, { t: "bigInt", v: "1" })).toBe(false);
    expect(valuesEqual({ t: "null" }, { t: "null" })).toBe(true);
  });
  it("produces parameter text", () => {
    expect(valueText({ t: "bool", v: false })).toBe("false");
    expect(valueText({ t: "null" })).toBeNull();
    expect(
      pgArrayLiteral([
        { t: "text", v: 'a"b' },
        { t: "int", v: 2 },
      ]),
    ).toBe('{"a\\"b","2"}');
  });
  it("detects line breaks", () => {
    expect(hasLineBreak("a\nb")).toBe(true);
    expect(hasLineBreak("a\r")).toBe(true);
    expect(hasLineBreak("a b")).toBe(false);
  });
  it("splits and merges the date part", () => {
    expect(splitDatePart("2024-03-05 12:00:00+07")).toEqual({
      date: "2024-03-05",
      rest: " 12:00:00+07",
    });
    expect(splitDatePart("infinity")).toEqual({ date: null, rest: "" });
    expect(mergeDatePart("2024-03-05T12:00:00Z", "2025-01-31")).toBe("2025-01-31T12:00:00Z");
    expect(mergeDatePart("2024-03-05", "2025-01-31")).toBe("2025-01-31");
    expect(mergeDatePart("", "2025-01-31")).toBe("2025-01-31");
    expect(mergeDatePart("now", "2025-01-31")).toBe("2025-01-31");
  });
});

describe("decodeBytesText (blob decoded to text when possible)", () => {
  it("decodes strict UTF-8, tab / line feed / carriage return included", () => {
    expect(decodeBytesText("SGVsbG8=")).toBe("Hello");
    expect(decodeBytesText(b64(utf8("naïve ✓ 日本")))).toBe("naïve ✓ 日本");
    expect(decodeBytesText(b64(utf8("a\tb\r\nc")))).toBe("a\tb\r\nc");
    expect(decodeBytesText("")).toBe("");
  });

  it("is null for invalid UTF-8, NUL and other control characters", () => {
    expect(decodeBytesText(b64([0xff, 0xfe, 0x41]))).toBeNull();
    // A lone continuation byte and an overlong sequence.
    expect(decodeBytesText(b64([0x80]))).toBeNull();
    expect(decodeBytesText(b64([0xc0, 0xaf]))).toBeNull();
    expect(decodeBytesText(b64([0x48, 0x00, 0x69]))).toBeNull();
    expect(decodeBytesText(b64([0x48, 0x1b, 0x5b]))).toBeNull();
    expect(decodeBytesText(b64([0x48, 0x7f]))).toBeNull();
    expect(decodeBytesText("not base64!")).toBeNull();
  });

  it("is null for a 16-byte binary id", () => {
    const id = [
      0x1f, 0x8b, 0x08, 0x00, 0xe4, 0x9a, 0x03, 0xc5, 0xfe, 0x12, 0x80, 0x77, 0x00, 0x42, 0xd1,
      0x9c,
    ];
    expect(decodeBytesText(b64(id))).toBeNull();
  });

  it("cuts a long blob at maxBytes without failing on a character cut in two", () => {
    const text = "aé日😀";
    const bytes = utf8(text); // 1 + 2 + 3 + 4 bytes
    expect(decodeBytesText(b64(bytes), 3)).toBe("aé");
    expect(decodeBytesText(b64(bytes), 4)).toBe("aé");
    expect(decodeBytesText(b64(bytes), 6)).toBe("aé日");
    expect(decodeBytesText(b64(bytes), 8)).toBe("aé日");
    expect(decodeBytesText(b64(bytes), 100)).toBe(text);
    // The cut is not an excuse for garbage before it.
    expect(decodeBytesText(b64([0x41, 0xff, 0x42]), 2)).toBeNull();
  });

  it("drops a cut-off last character of a preview only when told it is one", () => {
    const cutOff = utf8("ab日").slice(0, 4); // 'a', 'b', first byte of 日
    expect(decodeBytesText(b64(cutOff))).toBeNull();
    expect(decodeBytesText(b64(cutOff), undefined, true)).toBe("ab");
    // A preview that is invalid in the middle stays invalid.
    expect(decodeBytesText(b64([0x61, 0xff, 0x62]), undefined, true)).toBeNull();
  });
});

describe("displayValue of a blob", () => {
  it("shows a short whole blob that is text as that text, on one line", () => {
    expect(displayValue(blob(utf8("Hello")))).toEqual({ text: "Hello", cls: "bytes" });
    expect(displayValue(blob(utf8("a\n  b\tc"))).text).toBe("a b c");
    const long = "x".repeat(256);
    expect(displayValue(blob(utf8(long))).text).toBe(long);
    // One byte over the text limit: a large blob, read as its size.
    expect(displayValue(blob(utf8("x".repeat(257)))).text).toBe("<blob 257 B>");
  });

  it("shows a short whole blob that is not text as 0x + lowercase hex (BINARY(16) keys)", () => {
    const id = [
      0x00, 0x1a, 0xff, 0x80, 0xc3, 0x28, 0x7f, 0x01, 0xab, 0xcd, 0xef, 0x10, 0x20, 0x30, 0x40,
      0xfe,
    ];
    expect(id).toHaveLength(16);
    const d = displayValue(blob(id));
    expect(d).toEqual({ text: `0x${bytesHex(b64(id))}`, cls: "bytes" });
    expect(d.text).toMatch(/^0x[0-9a-f]{32}$/);
    const max = Array.from({ length: 32 }, (_, i) => 0x80 + i);
    expect(displayValue(blob(max)).text).toMatch(/^0x[0-9a-f]{64}$/);
  });

  it("keeps `<blob N B>` for a binary blob over 32 bytes, a truncated preview and an empty one", () => {
    const bin = (n: number) => Array.from({ length: n }, (_, i) => 0x80 + (i % 64));
    expect(displayValue(blob(bin(33))).text).toBe("<blob 33 B>");
    expect(displayValue(blob(bin(300))).text).toBe("<blob 300 B>");
    // A preview is never read as the value, text or not.
    expect(
      displayValue({ t: "bytes", v: { base64: b64(utf8("Hel")), len: 9000, truncated: true } }),
    ).toEqual({ text: "<blob 8.8 KB>", cls: "bytes" });
    expect(displayValue(blob(bin(4), true)).text).toBe("<blob 4 B>");
    expect(displayValue(blob([])).text).toBe("<blob 0 B>");
    // Blanks alone are not text worth showing.
    expect(displayValue(blob(utf8("   \n"))).text).toBe("0x2020200a");
  });

  it("reaches the grid cell as it is (cellDisplay), keeping the bytes class", () => {
    expect(cellDisplay(blob(utf8("Hello")))).toEqual({
      text: "Hello",
      cls: "bytes",
      placeholder: false,
    });
    expect(cellDisplay(blob([0xde, 0xad, 0xbe, 0xef])).text).toBe("0xdeadbeef");
  });
});
