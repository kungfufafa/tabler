import { useEffect } from "react";
import { useProfiles } from "@/features/connections/useConnections";
import { useReconnectInFlight } from "@/hooks/useConnectionEvents";
import { isTauri } from "@/lib/ipc";
import { useActiveConnection } from "@/stores/connections";
import { useActiveTab } from "@/stores/tabs";
import { useSessionDetails } from "./statusDetailsModel";
import { pillModel, windowTitle } from "./statusPillModel";

async function setNativeTitle(title: string): Promise<void> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTitle(title);
}

/**
 * Keeps the window's title on what it shows (`windowTitle`): the connection and the visible tab,
 * like TablePlus, so the taskbar and Alt+Tab tell windows apart. Mounted once by the app shell.
 */
export function WindowTitleHost({
  apply = setNativeTitle,
}: {
  apply?: (title: string) => Promise<void>;
}) {
  const active = useActiveConnection();
  const tab = useActiveTab();
  const profiles = useProfiles();
  const busy = useReconnectInFlight(active?.info.connectionId ?? null);
  const details = useSessionDetails(active);
  const tag = profiles.data?.find((p) => p.id === active?.info.profileId)?.options.tag ?? null;
  const title = windowTitle(
    active ? pillModel(active, tab, tag, busy, details?.tls ?? false) : null,
  );

  useEffect(() => {
    if (!isTauri()) {
      document.title = title;
      return;
    }
    apply(title).catch((e: unknown) => console.warn("[window] could not set the title", e));
  }, [title, apply]);
  return null;
}
