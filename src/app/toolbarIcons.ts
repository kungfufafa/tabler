import {
  Activity,
  ArrowUpFromLine,
  Database,
  Gauge,
  LayoutGrid,
  Lock,
  MoveHorizontal,
  PanelLeft,
  PanelsTopLeft,
  RefreshCw,
  Search,
  Space,
  UserCog,
  type LucideIcon,
} from "lucide-react";
import type { ToolbarItemId } from "./toolbarLayout";

/** The Customize Toolbar sheet's icon for each item. */
export const TOOLBAR_ITEM_ICON: Record<ToolbarItemId, LucideIcon> = {
  sidebar: PanelLeft,
  actions: ArrowUpFromLine,
  safeMode: Lock,
  databases: Database,
  status: Activity,
  refresh: RefreshCw,
  openAnything: Search,
  workspaces: LayoutGrid,
  users: UserCog,
  metrics: Gauge,
  panels: PanelsTopLeft,
  space: Space,
  flexibleSpace: MoveHorizontal,
};
