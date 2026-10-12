/** Decodes the first `maxBytes` bytes of a base64 string (without decoding the rest). */
export function base64Head(base64: string, maxBytes: number): Uint8Array {
  const chars = Math.ceil(maxBytes / 3) * 4;
  const bin = atob(base64.slice(0, chars).replace(/[^A-Za-z0-9+/=]/g, ""));
  const n = Math.min(bin.length, maxBytes);
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const hex2 = (b: number) => b.toString(16).padStart(2, "0");

/** Classic hex dump: `00000010  48 65 6c 6c 6f …  |Hello…|`, 16 bytes per line. */
export function hexDump(bytes: Uint8Array): string {
  const lines: string[] = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = bytes.subarray(off, off + 16);
    const hex = Array.from(chunk, hex2);
    const left = hex.slice(0, 8).join(" ");
    const right = hex.slice(8).join(" ");
    const ascii = Array.from(chunk, (b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."));
    lines.push(
      `${off.toString(16).padStart(8, "0")}  ${left.padEnd(23)}  ${right.padEnd(23)}  |${ascii.join("")}|`,
    );
  }
  return lines.join("\n");
}
