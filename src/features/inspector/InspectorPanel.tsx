import { Copy, X } from "lucide-react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import { ValueKindIcon } from "@/components/TypeIcon";
import { Button } from "@/components/ui/button";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { inspectText } from "@/lib/values";
import { useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";

export function InspectorPanel() {
  const cell = useInspectorStore((s) => s.cell);
  const toggle = useUiStore((s) => s.toggleInspector);
  return (
    <aside className="border-border bg-surface flex h-full flex-col border-l">
      <div className="border-border text-muted-foreground flex h-9 shrink-0 items-center gap-2 border-b px-3 text-xs font-semibold">
        Inspector
        <span className="flex-1" />
        <Button variant="ghost" size="icon-xs" onClick={toggle} aria-label="Close inspector">
          <X />
        </Button>
      </div>
      {cell ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="border-border flex items-center gap-2 border-b px-3 py-2 font-mono text-xs">
            <ValueKindIcon kind={cell.column.kind} />
            <span className="font-semibold">{cell.column.name}</span>
            <span className="text-muted-foreground">{cell.column.dataType}</span>
            <span className="flex-1" />
            <span className="text-muted-foreground text-[10px]">row {cell.rowIndex + 1}</span>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Copy value"
              onClick={() =>
                void writeClipboard(inspectText(cell.value)).then(() => toast.success("Copied"))
              }
            >
              <Copy />
            </Button>
          </div>
          <pre className="selectable min-h-0 flex-1 overflow-auto p-3 font-mono text-xs leading-5 break-words whitespace-pre-wrap">
            {inspectText(cell.value)}
          </pre>
        </div>
      ) : (
        <EmptyState
          title="No cell selected"
          description="Select a cell in the grid to see its full value."
        />
      )}
    </aside>
  );
}
