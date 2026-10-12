import { useEffect } from "react";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ConnectionDialogHost } from "@/features/connections/ConnectionDialogHost";
import { HostKeyPromptHost } from "@/features/connections/HostKeyPrompt";
import { Launcher } from "@/features/connections/Launcher";
import { ConsolePanel } from "@/features/console/ConsolePanel";
import { ExportHost } from "@/features/export";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { TabContent } from "@/features/tabs/TabContent";
import { TabStrip } from "@/features/tabs/TabStrip";
import { UnsavedChangesDialog } from "@/features/tabs/UnsavedChangesDialog";
import { useUnsavedGuard } from "@/hooks/useUnsavedGuard";
import { applyAccent, type AccentPreset } from "@/lib/theme";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionBanner } from "./ConnectionBanner";
import { Toolbar } from "./Toolbar";
import { WorkspaceRail } from "./WorkspaceRail";

/**
 * The connected workspace, TablePlus style: left sidebar | main column (tab bar, banner, the tab's
 * content, the console log under it) | right sidebar (row details).
 */
function Workspace() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  const consoleOpen = useUiStore((s) => s.consoleOpen);
  return (
    <ResizablePanelGroup orientation="horizontal" className="min-h-0 min-w-0 flex-1">
      {sidebarOpen && (
        <>
          <ResizablePanel id="sidebar" defaultSize={260} minSize={180} maxSize={480}>
            <Sidebar />
          </ResizablePanel>
          <ResizableHandle />
        </>
      )}
      <ResizablePanel id="main" minSize={360}>
        <div className="flex h-full min-h-0 flex-col">
          <TabStrip />
          <ConnectionBanner />
          {/* The console log (⇧⌘C) docks under the tab content. */}
          <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
            <ResizablePanel id="content" minSize={120}>
              <TabContent />
            </ResizablePanel>
            {consoleOpen && (
              <>
                <ResizableHandle />
                <ResizablePanel id="console" defaultSize={200} minSize={80} maxSize="75%">
                  <ConsolePanel />
                </ResizablePanel>
              </>
            )}
          </ResizablePanelGroup>
        </div>
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
  );
}

export function AppShell() {
  const view = useUiStore((s) => s.view);
  const active = useActiveConnection();
  const showLauncher = view === "launcher" || !active;
  // Window close / quit while tabs hold unsaved work (the dialog below asks).
  useUnsavedGuard();

  useEffect(() => {
    applyAccent((active?.info.color as AccentPreset | null) ?? null);
  }, [active?.info.color]);

  return (
    <div className="bg-background text-foreground flex h-screen w-screen flex-col overflow-hidden">
      <Toolbar launcher={showLauncher} />
      <div className="flex min-h-0 flex-1">
        <WorkspaceRail />
        {showLauncher ? (
          <main className="min-w-0 flex-1">
            <Launcher />
          </main>
        ) : (
          <Workspace />
        )}
      </div>
      {/* The one unsaved-changes confirmation for every close path (tabs, disconnect, window). */}
      <UnsavedChangesDialog />
      <CommandPalette />
      <SettingsDialog />
      {/* App-wide hosts: the export dialog (and its ⌘⇧E default) and the SSH host-key prompt. */}
      <ExportHost />
      <HostKeyPromptHost />
      <ConnectionDialogHost />
    </div>
  );
}
