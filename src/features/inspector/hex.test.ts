import { describe, expect, it } from "vitest";
import { base64Head, hexDump } from "./hex";

describe("hex", () => {
  it("decodes only the head of a base64 value", () => {
    const b64 = btoa("Hello, world! This is a longer value.");
    expect(Array.from(base64Head(b64, 5))).toEqual([72, 101, 108, 108, 111]);
    expect(base64Head(b64, 1000).length).toBe(37);
  });

  it("dumps 16 bytes per line with an ASCII column", () => {
    const bytes = new Uint8Array([
      ...Array.from("Hello, world!\n", (c) => c.charCodeAt(0)),
      0,
      255,
      1,
    ]);
    expect(hexDump(bytes).split("\n")).toEqual([
      "00000000  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 0a 00 ff  |Hello, world!...|",
      "00000010  01                                                |.|",
    ]);
  });
});
