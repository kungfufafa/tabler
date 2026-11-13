import { create } from "zustand";
import { ipc, type AppInfo } from "@/lib/ipc";

/**
 * What the backend says about this build (`app_info`): its version, and the drivers and
 * databases it has. Loaded once at boot; null until it answers (or if it cannot), when the
 * renderer assumes the core drivers only.
 */
export const useAppInfoStore = create<{ info: AppInfo | null }>()(() => ({ info: null }));

export function useAppInfo(): AppInfo | null {
  return useAppInfoStore((s) => s.info);
}

/**
 * "0.1.0-nightly.172 (1a2b3c4)": a build as the About dialog names it, its version and the commit
 * it was made from (a development build has none).
 */
export function buildLabel(info: Pick<AppInfo, "version" | "commit"> | null | undefined): string {
  if (!info) return "";
  return info.commit ? `${info.version} (${info.commit})` : info.version;
}

/** Asks the backend once, at boot (`Providers`). */
export async function loadAppInfo(): Promise<void> {
  try {
    useAppInfoStore.setState({ info: await ipc.appInfo() });
  } catch {
    // Keep null: the core drivers are in every build.
  }
}
