import { baseName } from "@/features/editor/files";
import { ipc, isMock } from "@/lib/ipc";
import { PluginCancelled } from "./execute";
import type { LoadedFile } from "./types";

/** Largest file `context.loadFile()` returns. */
export const MAX_LOAD_FILE_BYTES = 5 * 1024 * 1024;

const FILTERS = [
  {
    name: "Text files",
    extensions: ["txt", "csv", "tsv", "json", "jsonl", "sql", "md", "xml", "yaml", "yml", "log"],
  },
  { name: "All files", extensions: ["*"] },
];

/** The native Open panel (the browser mock has no file system and asks for a path). */
async function pickFile(): Promise<string | null> {
  if (isMock) {
    const path = window.prompt("Plugin — file to read", "/Users/me/Documents/data.csv");
    return path?.trim() || null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Choose a file for the plugin to read",
    multiple: false,
    directory: false,
    filters: FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

function fromBase64(base64: string): Uint8Array {
  const bin = atob(base64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

/**
 * The text of a file's bytes: UTF-8 only (a NUL byte or an invalid sequence means the file is not
 * text), at most `MAX_LOAD_FILE_BYTES`. A leading byte order mark is dropped.
 */
export function decodeLoadedFile(name: string, bytes: Uint8Array): string {
  if (bytes.length > MAX_LOAD_FILE_BYTES) throw new Error(`${name} is larger than 5 MB`);
  if (bytes.includes(0)) throw new Error(`${name} is not a text file`);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw new Error(`${name} is not UTF-8 text`);
  }
}

/**
 * `context.loadFile()`: the user picks a file in the native Open panel, the plugin gets its name
 * and text. The plugin never names a path. Cancelling the panel ends the command quietly, like
 * declining a link.
 */
export async function loadPluginFile(): Promise<LoadedFile> {
  const path = await pickFile();
  if (!path) throw new PluginCancelled("No file was chosen");
  const name = baseName(path);
  const value = await ipc.files.readBinary(path);
  if (value.t !== "bytes") throw new Error(`${name} could not be read`);
  const { base64, len, truncated } = value.v;
  // The size is known before the bytes are decoded: a big file is refused without touching it.
  if (truncated || len > MAX_LOAD_FILE_BYTES) throw new Error(`${name} is larger than 5 MB`);
  return { name, text: decodeLoadedFile(name, fromBase64(base64)) };
}
