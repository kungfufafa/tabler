import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { NotificationToaster } from "@/features/notify/NotificationToaster";
import { useDefaultActions } from "@/features/palette/defaultActions";
import { useConnectionEvents } from "@/hooks/useConnectionEvents";
import { useNativeMenu } from "@/hooks/useNativeMenu";
import { useTheme } from "@/hooks/useTheme";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";
import { installActiveSync } from "@/stores/activeSync";
import { loadAppInfo } from "@/stores/appInfo";

function GlobalEffects() {
  useTheme();
  useDefaultActions();
  useConnectionEvents();
  useNativeMenu();
  // The visible tab and the active connection (sidebar, status bar, banner, ⌘T) never drift apart.
  useEffect(() => installActiveSync(), []);
  // The drivers and databases this build has (the new-connection picker's tiles).
  useEffect(() => void loadAppInfo(), []);
  return null;
}

/**
 * The Settings window's share of `GlobalEffects`: the theme, the menu items it handles
 * (`menu:action`, see `menu_route` in src-tauri/src/windows.rs) and the build's info (General ▸
 * the running version). No tabs, connections or workspace actions.
 */
function AuxEffects() {
  useTheme();
  useNativeMenu();
  useEffect(() => void loadAppInfo(), []);
  return null;
}

export function Providers({
  children,
  aux = false,
}: {
  children: ReactNode;
  /** A window that is not an app window (the Settings window): only what it needs. */
  aux?: boolean;
}) {
  const [queryClient] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: { staleTime: 30_000, retry: false, refetchOnWindowFocus: false },
          mutations: { retry: false },
        },
      }),
  );
  return (
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delayDuration={400}>
        <KeyboardProvider>
          {aux ? <AuxEffects /> : <GlobalEffects />}
          {children}
          <NotificationToaster compact={aux} />
        </KeyboardProvider>
      </TooltipProvider>
    </QueryClientProvider>
  );
}
