import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ipc, type Value } from "@/lib/ipc";
import { resetPrefsForTests } from "@/stores/prefs";
import { handleBridgeCall } from "./bridge";
import { PluginCancelled } from "./execute";
import { createHostServices } from "./host";
import { decodeLoadedFile, loadPluginFile, MAX_LOAD_FILE_BYTES } from "./loadFile";
import { resetPluginsStoreForTests } from "./store";
import type { PluginInvocation } from "./types";

const inv: PluginInvocation = {
  source: "global",
  connectionId: null,
  driver: null,
  item: null,
  clickedRow: null,
  clickedColumn: null,
  selectedRows: [],
  editorTabId: null,
  sql: null,
};

const bytesValue = (bytes: Uint8Array, over: { len?: number; truncated?: boolean } = {}): Value => {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return {
    t: "bytes",
    v: { base64: btoa(bin), len: over.len ?? bytes.length, truncated: over.truncated ?? false },
  };
};
const utf8 = (text: string) => new TextEncoder().encode(text);

beforeEach(() => {
  resetPrefsForTests();
  resetPluginsStoreForTests();
});
afterEach(() => vi.restoreAllMocks());

/** The (mock) file dialog answers with `path`; null cancels it. */
const pick = (path: string | null) => vi.spyOn(window, "prompt").mockReturnValue(path);

describe("decodeLoadedFile", () => {
  it("decodes UTF-8 text and drops a byte order mark", () => {
    expect(decodeLoadedFile("a.csv", utf8("a,b\né,😀\n"))).toBe("a,b\né,😀\n");
    expect(decodeLoadedFile("a.csv", new Uint8Array([0xef, 0xbb, 0xbf, 0x61]))).toBe("a");
    expect(decodeLoadedFile("empty.txt", new Uint8Array())).toBe("");
  });

  it("refuses what is not text", () => {
    expect(() => decodeLoadedFile("x.bin", new Uint8Array([0x61, 0x00, 0x62]))).toThrow(
      "x.bin is not a text file",
    );
    // Latin-1 "é" (0xE9) is not valid UTF-8.
    expect(() => decodeLoadedFile("old.csv", new Uint8Array([0x61, 0xe9, 0x62]))).toThrow(
      "old.csv is not UTF-8 text",
    );
  });

  it("refuses more than 5 MB", () => {
    expect(MAX_LOAD_FILE_BYTES).toBe(5 * 1024 * 1024);
    expect(decodeLoadedFile("ok.txt", new Uint8Array(MAX_LOAD_FILE_BYTES).fill(0x61))).toHaveLength(
      MAX_LOAD_FILE_BYTES,
    );
    expect(() =>
      decodeLoadedFile("big.txt", new Uint8Array(MAX_LOAD_FILE_BYTES + 1).fill(0x61)),
    ).toThrow("larger than 5 MB");
  });
});

describe("loadPluginFile", () => {
  it("returns the name and text of the file the user picked", async () => {
    pick("/Users/me/Documents/people.csv");
    const read = vi
      .spyOn(ipc.files, "readBinary")
      .mockResolvedValue(bytesValue(utf8("id,name\n1,Ann\n")));
    expect(await loadPluginFile()).toEqual({ name: "people.csv", text: "id,name\n1,Ann\n" });
    // The path is the user's choice: it comes from the dialog, never from the plugin.
    expect(read).toHaveBeenCalledWith("/Users/me/Documents/people.csv");
  });

  it("ends the command quietly when the dialog is cancelled", async () => {
    pick(null);
    const read = vi.spyOn(ipc.files, "readBinary");
    await expect(loadPluginFile()).rejects.toBeInstanceOf(PluginCancelled);
    expect(read).not.toHaveBeenCalled();
  });

  it("refuses a file over 5 MB without decoding it", async () => {
    pick("/Users/me/big.log");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(
      bytesValue(utf8("x"), { len: MAX_LOAD_FILE_BYTES + 1 }),
    );
    await expect(loadPluginFile()).rejects.toThrow("big.log is larger than 5 MB");
  });

  it("refuses a binary file and an invalid encoding", async () => {
    pick("/Users/me/photo.png");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(
      bytesValue(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00])),
    );
    await expect(loadPluginFile()).rejects.toThrow("photo.png is not a text file");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(
      bytesValue(new Uint8Array([0xff, 0xfe, 0x41])),
    );
    await expect(loadPluginFile()).rejects.toThrow("is not UTF-8 text");
  });
});

describe("context.loadFile through the host", () => {
  const plugin = { identifier: "com.example.t", name: "T" };

  it("answers the bridge call with { name, text }", async () => {
    pick("/Users/me/notes.txt");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(bytesValue(utf8("hello")));
    const out = await handleBridgeCall(
      createHostServices({ plugin, invocation: inv }),
      "loadFile",
      [],
    );
    expect(out).toEqual({ name: "notes.txt", text: "hello" });
  });

  it("opens the dialog once per command run", async () => {
    const prompt = pick("/Users/me/notes.txt");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(bytesValue(utf8("hello")));
    const host = createHostServices({ plugin, invocation: inv });
    await host.loadFile();
    await expect(host.loadFile()).rejects.toThrow("once per command run");
    expect(prompt).toHaveBeenCalledTimes(1);
    // A new run (new services) may ask again.
    await createHostServices({ plugin, invocation: inv }).loadFile();
    expect(prompt).toHaveBeenCalledTimes(2);
  });

  it("counts as a dialog of the run and pauses its time-out while the user browses", async () => {
    pick("/Users/me/notes.txt");
    vi.spyOn(ipc.files, "readBinary").mockResolvedValue(bytesValue(utf8("hello")));
    const events: string[] = [];
    const host = createHostServices({
      plugin,
      invocation: inv,
      dialog: async (show) => {
        events.push("dialog");
        return show();
      },
    });
    await host.loadFile();
    expect(events).toEqual(["dialog"]);
  });

  it("counts a cancelled dialog as the run's one file dialog", async () => {
    pick(null);
    const host = createHostServices({ plugin, invocation: inv });
    await expect(host.loadFile()).rejects.toBeInstanceOf(PluginCancelled);
    // The plugin cannot ask again after a cancel: one dialog per run.
    await expect(host.loadFile()).rejects.toThrow("once per command run");
  });
});
