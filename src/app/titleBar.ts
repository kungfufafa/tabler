import { isMac } from "@/lib/keys/combo";
import { windowsChrome } from "@/lib/platform";

/**
 * macOS draws the page under an overlay title bar (tauri.conf.json `titleBarStyle: "Overlay"`):
 * the toolbar is the title bar and leaves room for the traffic lights.
 */
export const overlayTitleBar: boolean = isMac;

/**
 * Windows: like TablePlus for Windows, the window has no native title bar or menu bar (the backend
 * turns the decorations off). The toolbar is the title bar, with the ⋮ app menu at its left and
 * the minimize / maximize / close buttons at its right. Linux looks the same (`windowsChrome`).
 */
export const ownTitleBar: boolean = windowsChrome;
