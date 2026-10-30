import { Gauge } from "lucide-react";
import { useCallback } from "react";
import { useAction } from "@/features/palette/actions";
import { registerToolTab } from "@/features/tabs/toolTabs";
import { allowFeature } from "@/lib/caps";
import { useConnectionsStore } from "@/stores/connections";
import { MetricsBoardTab } from "./MetricsBoardTab";
import {
  METRICS_TITLE,
  METRICS_TOOL,
  metricsParams,
  openMetricsBoard,
  type MetricsParams,
} from "./open";

registerToolTab<MetricsParams>(METRICS_TOOL, {
  component: MetricsBoardTab,
  icon: Gauge,
  title: () => METRICS_TITLE,
  persist: true,
  restore: metricsParams,
});

/**
 * App-wide host of the Metrics Board: the `metrics.open` action (toolbar Metrics Board,
 * View ▸ Metrics Board, ⌘P "Metrics Board") opens the active connection's board tab.
 */
export function MetricsHost() {
  const activeId = useConnectionsStore((s) => s.activeId);
  const open = useCallback(() => {
    const id = useConnectionsStore.getState().activeId;
    if (id && allowFeature(id, "metrics")) openMetricsBoard(id);
  }, []);
  useAction("metrics.open", activeId ? open : null);
  return null;
}
