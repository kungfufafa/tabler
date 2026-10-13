import { useCallback, useEffect } from "react";
import { useAction } from "@/features/palette/actions";
import { useConnectionsStore } from "@/stores/connections";
import { DiagramDialog } from "./DiagramDialog";
import { closeDiagram, openDiagram, useDiagramStore } from "./store";

/**
 * App-wide host of Generate Diagram…: the dialog `openDiagram` asks for, and the palette's
 * "Generate Diagram…" (`diagram.open`, the active connection's current schema).
 */
export function DiagramHost() {
  const request = useDiagramStore((s) => s.request);
  const seq = useDiagramStore((s) => s.seq);
  const activeId = useConnectionsStore((s) => s.activeId);
  const connected = useConnectionsStore((s) =>
    request ? s.byId[request.connectionId] !== undefined : false,
  );

  const openForActive = useCallback(() => {
    const { activeId: id, byId } = useConnectionsStore.getState();
    if (!id) return;
    openDiagram({ connectionId: id, schema: byId[id]?.currentSchema ?? null, tables: null });
  }, []);
  useAction("diagram.open", activeId ? openForActive : null);

  // The connection went away (disconnect, closed workspace): nothing left to draw.
  useEffect(() => {
    if (request && !connected) closeDiagram();
  }, [request, connected]);

  if (!request || !connected) return null;
  return <DiagramDialog key={seq} request={request} onClose={closeDiagram} />;
}
