import { Table2 } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { QueryTab } from "@/features/editor/QueryTab";
import { StructureTab } from "@/features/structure/StructureTab";
import { TableTab } from "@/features/table-tab/TableTab";
import { useActiveTab } from "@/stores/tabs";

export function TabContent() {
  const tab = useActiveTab();
  if (!tab) {
    return (
      <EmptyState
        icon={Table2}
        title="No tab open"
        description="Pick a table in the sidebar, or press ⌘T for a new query."
      />
    );
  }
  switch (tab.kind) {
    case "table":
      return <TableTab key={tab.id} tab={tab} />;
    case "structure":
      return <StructureTab key={tab.id} tab={tab} />;
    case "query":
      return <QueryTab key={tab.id} tab={tab} />;
  }
}
