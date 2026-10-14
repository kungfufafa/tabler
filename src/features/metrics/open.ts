import { isMock } from "@/lib/ipc";
import { findToolTab, openToolTab } from "@/features/tabs/toolTabs";
import { useTabsStore } from "@/stores/tabs";

/** The Metrics Board's tool tab (`registerToolTab`, MetricsHost.tsx). */
export const METRICS_TOOL = "metrics";
export const METRICS_TITLE = "Metrics Board";

export interface MetricsParams {
  /** The board shown; null = the connection's first board. */
  boardId: string | null;
}

/** Saved / handed-over params of a Metrics Board tab (null when they are not usable). */
export function metricsParams(raw: unknown): MetricsParams | null {
  if (raw === null || raw === undefined) return { boardId: null };
  if (typeof raw !== "object") return null;
  const id = (raw as { boardId?: unknown }).boardId;
  if (id === null || id === undefined) return { boardId: null };
  return typeof id === "string" && id !== "" ? { boardId: id } : null;
}

/**
 * Shows the connection's Metrics Board tab (one per connection): on `boardId` when given, else
 * on the board it already shows (a new tab shows the first board).
 */
export function openMetricsBoard(connectionId: string, boardId?: string | null): string {
  const existing = findToolTab(
    useTabsStore.getState().tabs,
    connectionId,
    METRICS_TOOL,
    null,
    "tool",
  );
  const params: MetricsParams =
    boardId !== undefined ? { boardId } : (metricsParams(existing?.params) ?? { boardId: null });
  return openToolTab(connectionId, METRICS_TOOL, params, { reuse: "tool", title: METRICS_TITLE });
}

export const BOARD_FILE_EXTENSION = "tablerboard";
const FILTERS = [{ name: "Tabler board", extensions: [BOARD_FILE_EXTENSION] }];

/** Asks where to save a board file (save panel). */
export async function pickBoardSavePath(name: string): Promise<string | null> {
  const file = `${name.replace(/[/\\:*?"<>|]+/g, "-")}.${BOARD_FILE_EXTENSION}`;
  if (isMock) return window.prompt("Export board to", `/Users/me/Downloads/${file}`);
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ title: "Export Board", defaultPath: file, filters: FILTERS });
  return typeof picked === "string" ? picked : null;
}

/** Asks for a board file to import (open panel). */
export async function pickBoardOpenPath(): Promise<string | null> {
  if (isMock) return window.prompt("Import board from", "/Users/me/Downloads/sales.tablerboard");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Import Board",
    multiple: false,
    directory: false,
    filters: FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}
