import { isMac } from "@/lib/keys/combo";

/**
 * macOS draws the page under an overlay title bar (tauri.conf.json `titleBarStyle: "Overlay"`):
 * the toolbar is the title bar and leaves room for the traffic lights. Windows keeps its native
 * title bar and menu bar above the page, so the app adds no title strip and no inset.
 */
export const overlayTitleBar: boolean = isMac;
