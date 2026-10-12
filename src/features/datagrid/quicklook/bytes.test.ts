import { describe, expect, it } from "vitest";
import { sniffBase64Image, sniffImage } from "./bytes";

const b64 = (bytes: number[]) => btoa(String.fromCharCode(...bytes));
const ascii = (s: string) => Array.from(s, (c) => c.charCodeAt(0));

describe("sniffImage", () => {
  it("recognizes PNG, JPEG, GIF and WebP by their magic bytes", () => {
    expect(sniffImage(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0]))).toBe(
      "image/png",
    );
    expect(sniffImage(new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBe("image/jpeg");
    expect(sniffImage(new Uint8Array(ascii("GIF89a....")))).toBe("image/gif");
    expect(sniffImage(new Uint8Array(ascii("GIF87a")))).toBe("image/gif");
    expect(sniffImage(new Uint8Array([...ascii("RIFF"), 1, 2, 3, 4, ...ascii("WEBPVP8 ")]))).toBe(
      "image/webp",
    );
  });

  it("says null for anything else, short or truncated", () => {
    expect(sniffImage(new Uint8Array(ascii("hello world")))).toBeNull();
    expect(sniffImage(new Uint8Array([0x89, 0x50]))).toBeNull();
    expect(sniffImage(new Uint8Array([...ascii("RIFF"), 1, 2, 3, 4, ...ascii("WAVE")]))).toBeNull();
    expect(sniffBase64Image(b64([0xff, 0xd8, 0xff, 0xdb, 1, 2, 3]))).toBe("image/jpeg");
    expect(sniffBase64Image("not base64 at all!!")).toBeNull();
  });
});
