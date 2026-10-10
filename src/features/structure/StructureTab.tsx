import { TableProperties } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import type { Tab } from "@/stores/tabs";

/** Placeholder until the structure editor lands in M5. */
export function StructureTab({ tab }: { tab: Extract<Tab, { kind: "structure" }> }) {
  return (
    <EmptyState
      icon={TableProperties}
      title={`${tab.table.name} · structure`}
      description="Columns, indexes and foreign keys arrive in M5."
    />
  );
}
