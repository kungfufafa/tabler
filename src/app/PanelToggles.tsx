import { PanelBottom, PanelLeft, PanelRight } from "lucide-react";
import { useUiStore } from "@/stores/ui";
import { ToolbarButton } from "./ToolbarButton";

/**
 * The panel switches at the toolbar's right: console log and right sidebar, with the left sidebar
 * first unless the toolbar's own Toggle Sidebar item (TablePlus's leftmost button) already has it.
 */
export function PanelToggles({ leftSidebar = true }: { leftSidebar?: boolean }) {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const consoleOpen = useUiStore((s) => s.consoleOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  return (
    <div
      role="group"
      aria-label="Panels"
      data-slot="panel-toggles"
      className="border-border bg-surface flex h-8 shrink-0 items-center gap-0.5 rounded-md border p-0.5"
    >
      {leftSidebar && (
        <ToolbarButton
          label="Left sidebar"
          action="ui.toggle-sidebar"
          pressed={sidebarOpen}
          className="size-6"
        >
          <PanelLeft className="size-3.5" />
        </ToolbarButton>
      )}
      <ToolbarButton
        label="Console log"
        action="ui.toggle-console"
        pressed={consoleOpen}
        className="size-6"
      >
        <PanelBottom className="size-3.5" />
      </ToolbarButton>
      <ToolbarButton
        label="Right sidebar"
        action="ui.toggle-inspector"
        pressed={inspectorOpen}
        className="size-6"
      >
        <PanelRight className="size-3.5" />
      </ToolbarButton>
    </div>
  );
}
