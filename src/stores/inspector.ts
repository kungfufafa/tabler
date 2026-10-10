import { create } from "zustand";
import type { GridColumn } from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";

export interface InspectedCell {
  column: GridColumn;
  value: Value;
  rowIndex: number;
  source: string;
}

interface InspectorState {
  cell: InspectedCell | null;
  set: (cell: InspectedCell | null) => void;
}

export const useInspectorStore = create<InspectorState>()((set) => ({
  cell: null,
  set: (cell) => set({ cell }),
}));
