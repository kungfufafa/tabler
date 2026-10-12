import { base64Head } from "@/features/inspector/hex";

export type ImageMime = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

const startsWith = (bytes: Uint8Array, sig: readonly number[], at = 0) =>
  bytes.length >= at + sig.length && sig.every((b, i) => bytes[at + i] === b);

const ascii = (s: string) => Array.from(s, (c) => c.charCodeAt(0));

/** The image type a blob's magic bytes announce (PNG, JPEG, GIF, WebP), or null. */
export function sniffImage(bytes: Uint8Array): ImageMime | null {
  if (startsWith(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (startsWith(bytes, [0xff, 0xd8, 0xff])) return "image/jpeg";
  if (startsWith(bytes, ascii("GIF87a")) || startsWith(bytes, ascii("GIF89a"))) return "image/gif";
  if (startsWith(bytes, ascii("RIFF")) && startsWith(bytes, ascii("WEBP"), 8)) return "image/webp";
  return null;
}

/** The image type of a base64 blob (from its first bytes only). */
export function sniffBase64Image(base64: string): ImageMime | null {
  try {
    return sniffImage(base64Head(base64, 16));
  } catch {
    return null;
  }
}
