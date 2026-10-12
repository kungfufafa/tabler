import { PanelBottom, PanelLeft, PanelRight } from "lucide-react";
import { useUiStore } from "@/stores/ui";
import { ToolbarButton } from "./ToolbarButton";

/** The three panel switches at the toolbar's right: left sidebar, console log, right sidebar. */
export function PanelToggles() {
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const consoleOpen = useUiStore((s) => s.consoleOpen);
  const inspectorOpen = useUiStore((s) => s.inspectorOpen);
  return (
    <div
      role="group"
      aria-label="Panels"
      data-slot="panel-toggles"
      className="border-border bg-surface flex h-6.5 shrink-0 items-center gap-px rounded-md border p-px"
    >
      <ToolbarButton
        label="Left sidebar"
        action="ui.toggle-sidebar"
        pressed={sidebarOpen}
        className="size-5.5 rounded-[5px]"
      >
        <PanelLeft className="size-3.5" />
      </ToolbarButton>
      <ToolbarButton
        label="Console log"
        action="ui.toggle-console"
        pressed={consoleOpen}
        className="size-5.5 rounded-[5px]"
      >
        <PanelBottom className="size-3.5" />
      </ToolbarButton>
      <ToolbarButton
        label="Right sidebar"
        action="ui.toggle-inspector"
        pressed={inspectorOpen}
        className="size-5.5 rounded-[5px]"
      >
        <PanelRight className="size-3.5" />
      </ToolbarButton>
    </div>
  );
}
