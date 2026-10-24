import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
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

export function Providers({ children }: { children: ReactNode }) {
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
          <GlobalEffects />
          {children}
          <Toaster />
        </KeyboardProvider>
      </TooltipProvider>
    </QueryClientProvider>
  );
}
