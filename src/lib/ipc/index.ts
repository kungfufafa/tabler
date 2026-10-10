import type { AppInfo } from "./bindings/AppInfo";
import { invoke } from "./transport";

export { IpcError, isMock, isTauri, toIpcError } from "./transport";

/** Typed facade over every Tauri command. The renderer never calls `invoke` directly. */
export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),
};
