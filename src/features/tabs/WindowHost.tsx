import { useEffect } from "react";
import { useAction } from "@/features/palette/actions";
import {
  installConnectionClaims,
  moveTabToNewWindow,
  openNewWindow,
  splitRight,
} from "./windowHandoff";

const newWindow = () => void openNewWindow();
const moveTab = () => void moveTabToNewWindow();

/**
 * The window-level actions: File ▸ New Window (⌘N), Window ▸ Move Tab to New Window, View ▸
 * Split Right (⇧⌘D); and the connections this window owns. Mounted once by the app shell.
 */
export function WindowHost() {
  useAction("window.new", newWindow);
  useAction("window.move-tab", moveTab);
  useAction("workspace.split-right", splitRight);
  useEffect(() => installConnectionClaims(), []);
  return null;
}
