import { isMac } from "@/lib/keys/combo";

export const isWindows: boolean =
  typeof navigator !== "undefined" && /Win/.test(navigator.platform ?? navigator.userAgent);

export const isLinux: boolean =
  typeof navigator !== "undefined" &&
  /Linux/.test(navigator.platform ?? navigator.userAgent) &&
  !/Android/.test(navigator.userAgent ?? "");

/**
 * Windows and Linux share one window look, TablePlus for Windows's: no native title or menu bar,
 * the toolbar is the title bar (⋮ app menu, minimize / maximize / close), the same tabs and the
 * same Welcome window. macOS keeps its native traffic lights and menu bar.
 */
export const windowsChrome: boolean = isWindows || isLinux;

/** Where the backend's `keyring` keeps passwords on this system. */
export const SECRET_STORE: string = isMac
  ? "macOS Keychain"
  : isWindows
    ? "Windows Credential Manager"
    : "system keyring";

/** The file manager, as this system calls it ("Could not show it in …"). */
export const FILE_MANAGER: string = isMac
  ? "Finder"
  : isWindows
    ? "File Explorer"
    : "the file manager";

/** The menu item that shows a file in the file manager. */
export const REVEAL_LABEL: string = isMac
  ? "Show in Finder"
  : isWindows
    ? "Show in File Explorer"
    : "Open Containing Folder";

/** A desktop notification, as this system calls it. */
export const NOTIFICATION_NOUN: string = isMac
  ? "macOS notification"
  : isWindows
    ? "Windows notification"
    : "desktop notification";
