import {
  Binary,
  Braces,
  Brackets,
  Calendar,
  CalendarClock,
  Clock,
  Eye,
  Fingerprint,
  Hash,
  Layers,
  List,
  Table2,
  ToggleLeft,
  Type,
  type LucideIcon,
} from "lucide-react";
import type { TableKind, ValueKind } from "@/lib/ipc";
import { cn } from "@/lib/utils";

const TABLE_ICONS: Record<TableKind, LucideIcon> = {
  table: Table2,
  partitioned: Table2,
  foreignTable: Table2,
  view: Eye,
  systemView: Eye,
  materializedView: Layers,
};

export function TableIcon({ kind, className }: { kind: TableKind; className?: string }) {
  const Icon = TABLE_ICONS[kind] ?? Table2;
  return (
    <Icon
      className={cn(
        "size-3.5 shrink-0",
        kind === "table" ? "text-brand" : "text-muted-foreground",
        className,
      )}
    />
  );
}

const VALUE_ICONS: Partial<Record<ValueKind, LucideIcon>> = {
  int: Hash,
  bigInt: Hash,
  decimal: Hash,
  float: Hash,
  bool: ToggleLeft,
  text: Type,
  enum: List,
  set: List,
  bytes: Binary,
  bit: Binary,
  date: Calendar,
  time: Clock,
  timestamp: CalendarClock,
  timestampTz: CalendarClock,
  interval: Clock,
  uuid: Fingerprint,
  json: Braces,
  array: Brackets,
};

export function ValueKindIcon({ kind, className }: { kind: ValueKind; className?: string }) {
  const Icon = VALUE_ICONS[kind] ?? Type;
  return <Icon className={cn("text-muted-foreground size-3 shrink-0", className)} />;
}
