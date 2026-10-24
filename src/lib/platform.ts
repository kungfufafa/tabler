import { isMac } from "@/lib/keys/combo";

export const isWindows: boolean =
  typeof navigator !== "undefined" && /Win/.test(navigator.platform ?? navigator.userAgent);

/** Where the backend's `keyring` keeps passwords on this system. */
export const SECRET_STORE: string = isMac
  ? "macOS Keychain"
  : isWindows
    ? "Windows Credential Manager"
    : "system keyring";
