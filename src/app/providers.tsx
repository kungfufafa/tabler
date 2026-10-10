import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useDefaultActions } from "@/features/palette/defaultActions";
import { useConnectionEvents } from "@/hooks/useConnectionEvents";
import { useTheme } from "@/hooks/useTheme";
import { KeyboardProvider } from "@/lib/keys/useShortcuts";

function GlobalEffects() {
  useTheme();
  useDefaultActions();
  useConnectionEvents();
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
