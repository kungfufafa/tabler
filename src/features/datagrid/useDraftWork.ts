import { useEffect } from "react";
import { registerWindowWork } from "@/stores/tabs";

/**
 * An edit of a cell that is not staged yet (an open editor's text): until it is, it is the
 * window's own unsaved work, so closing the window or quitting asks first instead of dropping it.
 */
export function useDraftWork(dirty: boolean, colName: string): void {
  useEffect(() => {
    if (!dirty) return;
    return registerWindowWork(() => [{ id: "cell-editor", title: `Edit of ${colName}`, count: 1 }]);
  }, [dirty, colName]);
}
