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

/** Asks the backend once, at boot (`Providers`). */
export async function loadAppInfo(): Promise<void> {
  try {
    useAppInfoStore.setState({ info: await ipc.appInfo() });
  } catch {
    // Keep null: the core drivers are in every build.
  }
}
