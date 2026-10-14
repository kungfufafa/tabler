import type { BuiltinPlugin } from "../types";

/** A version 4 (random) UUID from 16 random bytes (`crypto.getRandomValues` by default). */
export function uuidV4(bytes: Uint8Array = crypto.getRandomValues(new Uint8Array(16))): string {
  if (bytes.length !== 16) throw new Error("uuidV4 needs 16 bytes");
  const b = Uint8Array.from(bytes);
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const hex = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Where the UUID goes: the editor (SQL tabs), the clicked cell (as a staged edit), or the clipboard. */
export type UuidTarget = "editor" | "cell" | "clipboard";

export function uuidTarget(inv: {
  source: string;
  editorTabId: string | null;
  stageClickedCell?: unknown;
}): UuidTarget {
  if (inv.source === "grid") return inv.stageClickedCell ? "cell" : "clipboard";
  if (inv.editorTabId) return "editor";
  return inv.stageClickedCell ? "cell" : "clipboard";
}

export const UUID_GENERATOR: BuiltinPlugin = {
  identifier: "tabler.uuid-generator",
  name: "UUID Generator",
  version: "1.0",
  author: "Tabler",
  description:
    "Inserts a random (v4) UUID at the SQL editor's cursor, or into the selected cell as a staged edit.",
  commands: [
    {
      name: "Insert UUID",
      contexts: ["editor", "grid"],
      run: async (inv, host) => {
        const uuid = uuidV4();
        switch (uuidTarget(inv)) {
          case "editor":
            await host.insertToEditor(uuid);
            return;
          case "cell": {
            const col =
              inv.clickedRow && inv.clickedColumn !== null
                ? inv.clickedRow.columns[inv.clickedColumn]
                : undefined;
            // Only a UUID or text column takes it; any other cell gets nothing staged.
            if (col?.kind === "uuid" || col?.kind === "text") {
              inv.stageClickedCell?.(
                col.kind === "uuid" ? { t: "uuid", v: uuid } : { t: "text", v: uuid },
              );
              return;
            }
            await host.insertToClipboard(uuid);
            host.notify("UUID copied", `${col?.name ?? "The cell"} does not hold text or UUIDs.`);
            return;
          }
          case "clipboard":
            await host.insertToClipboard(uuid);
            host.notify("UUID copied", uuid);
        }
      },
    },
  ],
};
