import { Kbd } from "@/components/Kbd";
import { QueryTab } from "@/features/editor/QueryTab";
import { StructureTab } from "@/features/structure/StructureTab";
import { TableTab } from "@/features/table-tab/TableTab";
import { shortcutFor } from "@/lib/keys/keymap";
import { useActiveTab } from "@/stores/tabs";

/** The connected workspace before any table opens (TablePlus shows a single centred hint). */
function WorkspaceHint() {
  const openAnything = shortcutFor("palette.open") ?? "Mod+P";
  return (
    <div
      data-slot="workspace-hint"
      className="text-muted-foreground flex h-full items-center justify-center p-6 text-center text-xs"
    >
      <p>
        Select an item in the sidebar, or press{" "}
        <Kbd combo={openAnything} className="mx-0.5 align-middle" /> to open anything
      </p>
    </div>
  );
}

export function TabContent() {
  const tab = useActiveTab();
  if (!tab) return <WorkspaceHint />;
  switch (tab.kind) {
    case "table":
      return <TableTab key={tab.id} tab={tab} />;
    case "structure":
      return <StructureTab key={tab.id} tab={tab} />;
    case "query":
      return <QueryTab key={tab.id} tab={tab} />;
  }
}
