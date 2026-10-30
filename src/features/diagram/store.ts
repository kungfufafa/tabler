import { create } from "zustand";
import { allowFeature } from "@/lib/caps";
import type { TableRef } from "@/lib/ipc";

export interface DiagramRequest {
  connectionId: string;
  /** Schema whose tables are drawn; null = the sidebar's current schema (else the default). */
  schema: string | null;
  /**
   * Tables to start from: they are selected and shown with their neighbours ("Show only selected
   * tables + neighbours" on). Null = every table of the schema.
   */
  tables: TableRef[] | null;
}

interface DiagramState {
  request: DiagramRequest | null;
  /** Bumped by every `openDiagram`, so opening again starts a fresh dialog. */
  seq: number;
}

export const useDiagramStore = create<DiagramState>()(() => ({ request: null, seq: 0 }));

/** Sidebar / palette "Generate Diagram…" (selected tables, or the whole schema). */
export function openDiagram(request: DiagramRequest): void {
  if (!allowFeature(request.connectionId, "diagram")) return;
  useDiagramStore.setState((s) => ({ request, seq: s.seq + 1 }));
}

export function closeDiagram(): void {
  useDiagramStore.setState({ request: null });
}
