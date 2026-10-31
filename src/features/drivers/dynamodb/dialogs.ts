import { createElement, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import type { TableRef } from "@/lib/ipc";
import type { IndexChoice, TableInfo } from "./info";
import { IndexBrowserDialog, TableInfoDialog } from "./TableDialogs";

/**
 * Opens the dialogs of a table's menu. A menu item has no place in the tree of its own: each
 * dialog opens in a root of its own and is removed once closed.
 */
function openDialog(render: (close: () => void) => ReactElement): void {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const close = () =>
    // Unmounted after the closing animation's frame.
    setTimeout(() => {
      root.unmount();
      host.remove();
    }, 200);
  root.render(render(close));
}

/** Opens the table info panel of `table` with what `tableInfo` answered. */
export function openTableInfo(table: string, info: TableInfo): void {
  openDialog((close) => createElement(TableInfoDialog, { table, info, close }));
}

/** Opens the index browser of `table`. */
export function openIndexBrowser(
  connectionId: string,
  table: TableRef,
  indexes: readonly IndexChoice[],
): void {
  openDialog((close) => createElement(IndexBrowserDialog, { connectionId, table, indexes, close }));
}
