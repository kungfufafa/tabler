import { isMac } from "@/lib/keys/combo";

export const isWindows: boolean =
  typeof navigator !== "undefined" && /Win/.test(navigator.platform ?? navigator.userAgent);

/** Where the backend's `keyring` keeps passwords on this system. */
export const SECRET_STORE: string = isMac
  ? "macOS Keychain"
  : isWindows
    ? "Windows Credential Manager"
    : "system keyring";

/** The file manager, as this system calls it ("Could not show it in …"). */
export const FILE_MANAGER: string = isMac ? "Finder" : isWindows ? "File Explorer" : "file manager";

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
