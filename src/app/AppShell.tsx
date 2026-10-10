import { useEffect } from "react";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Launcher } from "@/features/connections/Launcher";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { StatusBar } from "@/features/statusbar/StatusBar";
import { TabContent } from "@/features/tabs/TabContent";
import { applyAccent, type AccentPreset } from "@/lib/theme";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { Titlebar } from "./Titlebar";

export function AppShell() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  const view = useUiStore((s) => s.view);
  const active = useActiveConnection();
  const showLauncher = view === "launcher" || !active;

  useEffect(() => {
    applyAccent((active?.info.color as AccentPreset | null) ?? null);
  }, [active?.info.color]);

  return (
    <div className="bg-background text-foreground flex h-screen w-screen flex-col overflow-hidden">
      <Titlebar />
      <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
        {sidebarOpen && (
          <>
            <ResizablePanel id="sidebar" defaultSize={260} minSize={180} maxSize={480}>
              <Sidebar />
            </ResizablePanel>
            <ResizableHandle />
          </>
        )}
        <ResizablePanel id="main" minSize={360}>
          {showLauncher ? <Launcher /> : <TabContent />}
        </ResizablePanel>
        {inspectorOpen && (
          <>
            <ResizableHandle />
            <ResizablePanel id="inspector" defaultSize={300} minSize={220} maxSize={600}>
              <InspectorPanel />
            </ResizablePanel>
          </>
        )}
      </ResizablePanelGroup>
      <StatusBar />
    </div>
  );
}
