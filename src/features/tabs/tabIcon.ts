import { FileCode2, LayoutGrid, Table2, TableProperties, type LucideIcon } from "lucide-react";
import type { Tab } from "@/stores/tabs";
import { getToolTab } from "./toolTabs";

const ICONS = { table: Table2, structure: TableProperties, query: FileCode2 } as const;

/** A tab's icon: its kind's, or the one its tool registered. */
export function tabIcon(tab: Tab): LucideIcon {
  if (tab.kind === "tool") return getToolTab(tab.tool)?.icon ?? LayoutGrid;
  return ICONS[tab.kind];
}
