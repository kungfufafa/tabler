import type { PlanState, ResultPane } from "@/stores/queryTabs";
import type { ResultSetInfo } from "./runModel";

/**
 * The pane the results show: the stored choice when it still exists, otherwise the first result
 * set, or the messages.
 */
export function resolvePane(
  pane: ResultPane,
  results: readonly ResultSetInfo[],
  plan: PlanState | null,
): ResultPane {
  if (pane === "plan") return plan ? "plan" : (results[0]?.id ?? "messages");
  if (pane === "messages") return "messages";
  return results.some((r) => r.id === pane) ? pane : (results[0]?.id ?? "messages");
}
