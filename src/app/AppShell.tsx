import { useEffect } from "react";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { AdminHost } from "@/features/admin/AdminHost";
import { AppearanceHost } from "@/features/appearance";
import { AssistantHost } from "@/features/assistant/AssistantHost";
import { BackupHost } from "@/features/backup/BackupHost";
import { ConnectionDialogHost } from "@/features/connections/ConnectionDialogHost";
import { ConnectionExtrasHost } from "@/features/connections/EncodingDialog";
import { HostKeyPromptHost } from "@/features/connections/HostKeyPrompt";
import { useKeepAlive } from "@/features/connections/keepAlive";
import { Launcher } from "@/features/connections/Launcher";
import { SshPromptHost } from "@/features/connections/SshPrompt";
import { ConsolePanel } from "@/features/console/ConsolePanel";
import { GeometryDecoderSync } from "@/features/datagrid/geometry/DecoderSync";
import { DeepLinkHost } from "@/features/deeplink/DeepLinkHost";
import { DiagramHost } from "@/features/diagram/DiagramHost";
import { ExportHost } from "@/features/export";
import { ImportHost } from "@/features/import";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { NotifyHost } from "@/features/notify/NotifyHost";
import { LockHost } from "@/features/lock/LockHost";
import { LocationsHost } from "@/features/locations/LocationsHost";
import { MetricsHost } from "@/features/metrics/MetricsHost";
import { ObjectsHost } from "@/features/objects/ObjectsHost";
import { CommandPalette } from "@/features/palette/CommandPalette";
import { PluginsHost } from "@/features/plugins/PluginsHost";
import { QueriesHost } from "@/features/queries/QueriesHost";
import { RecentHost } from "@/features/recent/RecentHost";
import { RerunConfirmDialog } from "@/features/results/RerunConfirmDialog";
import { SafetyHost } from "@/features/safety/SafetyHost";
import { SettingsDialog } from "@/features/settings/SettingsDialog";
import { Sidebar } from "@/features/sidebar/Sidebar";
import { SqlViewerHost } from "@/features/sqlviewer/SqlViewerHost";
import { SessionHost } from "@/features/tabs/SessionHost";
import { TabStrip } from "@/features/tabs/TabStrip";
import { UnsavedChangesDialog } from "@/features/tabs/UnsavedChangesDialog";
import { WindowHost } from "@/features/tabs/WindowHost";
import { WorkspaceGroups } from "@/features/tabs/WorkspaceGroups";
import { useActiveSplit } from "@/features/tabs/workspaces";
import { WorkspaceSwitcherHost } from "@/features/tabs/WorkspaceSwitcher";
import { useUnsavedGuard } from "@/hooks/useUnsavedGuard";
import { useKeymapVersion } from "@/lib/keys/keymap";
import { applyAccent, type AccentPreset } from "@/lib/theme";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionBanner } from "./ConnectionBanner";
import { Toolbar } from "./Toolbar";
import { WorkspaceRail } from "./WorkspaceRail";

/**
 * The connected workspace, TablePlus style: left sidebar | main column (tab bar, banner, the tab's
 * content, the console log under it) | right sidebar (row details). A split workspace (Split
 * Right) shows two tab groups, each with its own tab bar, under the banner.
 */
function Workspace() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  const consoleOpen = useUiStore((s) => s.consoleOpen);
  const split = useActiveSplit();
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
          {!split && <TabStrip />}
          <ConnectionBanner />
          {/* The console log (⇧⌘C) docks under the tab content. */}
          <ResizablePanelGroup orientation="vertical" className="min-h-0 flex-1">
            <ResizablePanel id="content" minSize={120}>
              <WorkspaceGroups />
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
  // A remapped shortcut re-renders the shell, so menus and tooltips show the new keys.
  useKeymapVersion();
  // Settings ▸ Connections ▸ Keep connection alive (ping every 30 s).
  useKeepAlive();

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
      {/* Settings ▸ Appearance: theme, fonts and colours, applied to the whole window. */}
      <AppearanceHost />
      {/* App-wide hosts: the export dialog (and its ⌘⇧E default) and the SSH host-key prompt. */}
      <ExportHost />
      {/* File ▸ Import ▸ From CSV… / From SQL Dump…. */}
      <ImportHost />
      {/* File ▸ Backup… / Restore… (pg_dump, mysqldump…). */}
      <BackupHost />
      <NotifyHost />
      <HostKeyPromptHost />
      {/* SSH keyboard-interactive (2FA) questions during connects and tests. */}
      <SshPromptHost />
      <DeepLinkHost />
      <ConnectionDialogHost />
      {/* Connection ▸ View Using Encoding…, and Settings ▸ General ▸ "At launch". */}
      <ConnectionExtrasHost />
      {/* Process List…, User Management…, the database / schema dialogs. */}
      <AdminHost />
      <QueriesHost />
      {/* Settings ▸ Locations startup warnings and the "Keep history for" pruning. */}
      <LocationsHost />
      {/* Sidebar object dialogs (delete / truncate / clone…) and the Item Overview. */}
      <ObjectsHost />
      {/* Generate Diagram… (sidebar menus, the palette). */}
      <DiagramHost />
      {/* Metrics Board (toolbar, View ▸ Metrics Board, the palette). */}
      <MetricsHost />
      {/* Safe-mode alerts and password prompts (guardExecution). */}
      <SafetyHost />
      {/* Session restore / auto-save, and Show Workspaces (⇧⌘\). */}
      <SessionHost />
      <WorkspaceSwitcherHost />
      {/* New Window (⌘N), Move Tab to New Window, Split Right (⇧⌘D); this window's connections. */}
      <WindowHost />
      {/* "Discard N uncommitted result changes and run?" before a SQL editor run. */}
      <RerunConfirmDialog />
      {/* Settings ▸ Data Decoder ▸ spatial values as WKT (sent to the backend). */}
      <GeometryDecoderSync />
      {/* The app lock (Settings ▸ Security ▸ Passcode, Window ▸ Lock Tabler ⌃⌘L). */}
      <LockHost />
      {/* File ▸ Open Recent: records what opens, opens what the menu picks. */}
      <RecentHost />
      {/* File ▸ New SQL Viewer (⇧⌘O): viewer tabs, and the standalone viewer with no connection. */}
      <SqlViewerHost />
      {/* Plugins ▸ Manage Plugins… (⌘L), the plugins' commands, menus and dialogs. */}
      <PluginsHost />
      {/* The palette's "Ask Assistant…" (right sidebar, Assistant tab). */}
      <AssistantHost />
    </div>
  );
}
