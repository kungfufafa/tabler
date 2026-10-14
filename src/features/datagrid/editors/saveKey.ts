import { actions } from "@/features/palette/actions";
import { comboFromEvent } from "@/lib/keys/combo";

/**
 * ⌘S (Ctrl+S elsewhere) in a cell editor. The grid's `grid.commit` key is not available while an
 * editor has the focus (`notWhileEditing`: its edit is not staged yet), so the editors take the key
 * themselves: they stage what was typed, then commit the tab (`commitTabSoon`), as the Details
 * fields and the structure cells do.
 */
export function isSaveKey(
  e: Pick<KeyboardEvent, "code" | "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
): boolean {
  return comboFromEvent(e) === "Mod+S";
}

/**
 * Commits the active tab's staged changes (Commit changes) once the edit that was just staged has
 * re-rendered it: running it in the same tick would commit the tab without that edit.
 */
export function commitTabSoon(): void {
  window.setTimeout(() => void actions.run("grid.commit"), 0);
}
