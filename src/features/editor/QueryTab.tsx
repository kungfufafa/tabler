import { FileCode2 } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import type { Tab } from "@/stores/tabs";

/** Placeholder until the SQL editor lands in M4. */
export function QueryTab({ tab }: { tab: Extract<Tab, { kind: "query" }> }) {
  return (
    <EmptyState
      icon={FileCode2}
      title={tab.title}
      description="The SQL editor arrives in M4."
      className="selectable"
    />
  );
}
