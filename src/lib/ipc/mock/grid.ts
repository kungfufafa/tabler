import type { Value } from "../bindings/Value";
import { mockError, registerMock } from "./transport";

/** A 24×24 PNG (blue / green checks): what a picked `.png` file holds in the mock. */
const SAMPLE_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAIAAABvFaqvAAAALklEQVR42mOwbvqGhgR2NqIhYtQwjBpER4PI04apZtQgeho0mrJH89qoQXQxCABVWFzOhm9OEgAAAABJRU5ErkJggg==";

const bytesValue = (bytes: Uint8Array): Value => {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return { t: "bytes", v: { base64: btoa(bin), len: bytes.length, truncated: false } };
};

/**
 * Set Value ▸ Add a File… (`read_binary_file`): the mock has no file system, so an image path
 * reads as a small PNG and any other path as its own UTF-8 text; a path with "missing" fails like
 * an unreadable file.
 */
registerMock("read_binary_file", ({ path }) => {
  const p = String(path ?? "");
  if (p.includes("missing")) mockError("IO", `${p}: No such file or directory (os error 2)`);
  if (/\.(png|jpe?g|gif|webp)$/i.test(p)) {
    const bin = atob(SAMPLE_PNG);
    return bytesValue(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
  }
  return bytesValue(new TextEncoder().encode(`contents of ${p}\n`));
});
