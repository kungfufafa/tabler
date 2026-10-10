import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { Launcher } from "@/features/connections/Launcher";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { StatusBar } from "@/features/statusbar/StatusBar";
import { useUiStore } from "@/stores/ui";
import { Titlebar } from "./Titlebar";

export function AppShell() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  return (
    <div className="bg-background text-foreground flex h-screen w-screen flex-col overflow-hidden">
      <Titlebar />
      <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
        {sidebarOpen && (
          <>
            <ResizablePanel id="sidebar" defaultSize={20} minSize={14} maxSize={40}>
              <Sidebar />
            </ResizablePanel>
            <ResizableHandle />
          </>
        )}
        <ResizablePanel id="main" minSize={30}>
          <Launcher />
        </ResizablePanel>
        {inspectorOpen && (
          <>
            <ResizableHandle />
            <ResizablePanel id="inspector" defaultSize={22} minSize={14} maxSize={45}>
              <aside className="border-border bg-surface h-full border-l" />
            </ResizablePanel>
          </>
        )}
      </ResizablePanelGroup>
      <StatusBar />
    </div>
  );
}
