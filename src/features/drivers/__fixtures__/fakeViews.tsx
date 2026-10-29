import type {
  InspectorSlotProps,
  KindFieldsProps,
  StructureViewProps,
  TableViewProps,
} from "../types";

// The fake lane's components (`fakeLane.tsx`): each shows that the app rendered it, and with what.

export function FakeFields({ kind }: KindFieldsProps) {
  return <p data-testid="fake-fields">{`Options of ${kind}`}</p>;
}

export function FakeTableView({ table }: TableViewProps) {
  return <div data-testid="fake-table-view">{`Documents of ${table.name}`}</div>;
}

export function FakeInspector({ row }: InspectorSlotProps) {
  return <pre data-testid="fake-inspector">{row ? JSON.stringify(row) : "No document"}</pre>;
}

export function FakeStructureView({ table }: StructureViewProps) {
  return <div data-testid="fake-structure">{`Indexes of ${table.name}`}</div>;
}
