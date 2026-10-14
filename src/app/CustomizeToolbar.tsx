import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  pointerWithin,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  horizontalListSortingStrategy,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { X } from "lucide-react";
import { useState, type KeyboardEvent, type PointerEventHandler } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { setPref, usePrefs } from "@/stores/prefs";
import { TOOLBAR_ITEM_ICON } from "./toolbarIcons";
import {
  DEFAULT_TOOLBAR_ITEMS,
  insertItem,
  moveItem,
  paletteItems,
  removeItemAt,
  TOOLBAR_DISPLAY_LABEL,
  TOOLBAR_DISPLAYS,
  TOOLBAR_ITEM_LABEL,
  type ToolbarDisplay,
  type ToolbarItemId,
} from "./toolbarLayout";

const STRIP = "toolbar-strip";
const PALETTE = "toolbar-palette";
const paletteId = (id: ToolbarItemId) => `palette:${id}`;

function slotKeys(items: readonly ToolbarItemId[]): string[] {
  const seen = new Map<ToolbarItemId, number>();
  return items.map((id) => {
    const n = seen.get(id) ?? 0;
    seen.set(id, n + 1);
    return `slot:${id}:${n}`;
  });
}

function Chip({ id, className }: { id: ToolbarItemId; className?: string }) {
  const Icon = TOOLBAR_ITEM_ICON[id];
  return (
    <span
      className={cn(
        "border-border bg-surface text-foreground flex h-7 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[11px] whitespace-nowrap shadow-xs",
        id === "status" && "min-w-32 justify-center",
        id === "flexibleSpace" && "border-dashed",
        className,
      )}
    >
      <Icon className="text-muted-foreground size-3.5" aria-hidden />
      {TOOLBAR_ITEM_LABEL[id]}
    </span>
  );
}

/** One item of the toolbar strip: sortable, removable with ⌫ / its ✕, or by dragging it out. */
function StripItem({
  slot,
  id,
  onRemove,
}: {
  slot: string;
  id: ToolbarItemId;
  onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: slot,
  });
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      onRemove();
      return;
    }
    listeners?.onKeyDown?.(e);
  };
  return (
    <div
      ref={setNodeRef}
      {...attributes}
      {...listeners}
      role="listitem"
      aria-label={TOOLBAR_ITEM_LABEL[id]}
      data-item={id}
      style={{ transform: CSS.Translate.toString(transform), transition }}
      className={cn(
        "group/chip focus-visible:ring-ring/50 relative rounded-md outline-none focus-visible:ring-2",
        isDragging && "opacity-40",
      )}
      onKeyDown={onKeyDown}
    >
      <Chip id={id} />
      <button
        type="button"
        aria-label={`Remove ${TOOLBAR_ITEM_LABEL[id]}`}
        tabIndex={-1}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={onRemove}
        className="bg-muted-foreground text-background absolute -top-1.5 -right-1.5 hidden size-3.5 items-center justify-center rounded-full group-hover/chip:flex"
      >
        <X className="size-2.5" aria-hidden />
      </button>
    </div>
  );
}

/** One palette item: drag it into the toolbar, or double-click / ↩ to add it at the end. */
function PaletteItem({ id, onAdd }: { id: ToolbarItemId; onAdd: () => void }) {
  const { setNodeRef, listeners, isDragging } = useDraggable({ id: paletteId(id) });
  return (
    <button
      ref={setNodeRef}
      type="button"
      data-item={id}
      aria-label={`Add ${TOOLBAR_ITEM_LABEL[id]}`}
      title="Double-click or drag into the toolbar"
      onPointerDown={listeners?.onPointerDown as PointerEventHandler | undefined}
      onDoubleClick={onAdd}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onAdd();
        }
      }}
      className={cn(
        "hover:bg-muted focus-visible:ring-ring/50 flex min-w-0 flex-col items-center gap-1 rounded-md px-1.5 py-2 text-center outline-none focus-visible:ring-2",
        isDragging && "opacity-40",
      )}
    >
      {(() => {
        const Icon = TOOLBAR_ITEM_ICON[id];
        return (
          <span className="border-border bg-surface flex size-8 items-center justify-center rounded-md border">
            <Icon className="text-muted-foreground size-4" aria-hidden />
          </span>
        );
      })()}
      <span className="text-[11px] leading-tight">{TOOLBAR_ITEM_LABEL[id]}</span>
    </button>
  );
}

function Strip({
  items,
  setItems,
}: {
  items: readonly ToolbarItemId[];
  setItems: (next: ToolbarItemId[]) => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: STRIP });
  const slots = slotKeys(items);
  return (
    <SortableContext items={slots} strategy={horizontalListSortingStrategy}>
      <div
        ref={setNodeRef}
        role="list"
        aria-label="Toolbar items"
        data-slot="toolbar-strip"
        className={cn(
          "border-border bg-surface-2 flex min-h-11 flex-wrap items-center gap-1.5 rounded-lg border px-2 py-2",
          isOver && "border-brand",
        )}
      >
        {items.length === 0 && (
          <span className="text-muted-foreground px-1 text-[11px]">
            Drag items here to add them to the toolbar.
          </span>
        )}
        {items.map((id, i) => (
          <StripItem
            key={slots[i]}
            slot={slots[i]!}
            id={id}
            onRemove={() => setItems(removeItemAt(items, i))}
          />
        ))}
      </div>
    </SortableContext>
  );
}

function Palette({ items, onAdd }: { items: ToolbarItemId[]; onAdd: (id: ToolbarItemId) => void }) {
  const { setNodeRef, isOver } = useDroppable({ id: PALETTE });
  return (
    <div
      ref={setNodeRef}
      data-slot="toolbar-palette"
      className={cn(
        "border-border grid grid-cols-5 gap-1 rounded-lg border border-dashed p-2",
        isOver && "border-destructive/60",
      )}
    >
      {items.map((id) => (
        <PaletteItem key={id} id={id} onAdd={() => onAdd(id)} />
      ))}
    </div>
  );
}

/**
 * Right-click the toolbar ▸ Customize Toolbar… (Safari / TablePlus): drag items between the
 * toolbar strip and the palette (out of the strip removes them), reorder the strip, Restore
 * Default Set, and Show: Icon Only / Icon and Text. Changes apply as they are made.
 */
export function CustomizeToolbarDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const items = usePrefs((p) => p.toolbarItems);
  const display = usePrefs((p) => p.toolbarDisplay);
  const [dragging, setDragging] = useState<ToolbarItemId | null>(null);
  const setItems = (next: ToolbarItemId[]) => setPref("toolbarItems", next);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const slots = slotKeys(items);

  const itemOf = (dragId: string): ToolbarItemId | null => {
    if (dragId.startsWith("palette:")) return dragId.slice("palette:".length) as ToolbarItemId;
    const i = slots.indexOf(dragId);
    return i >= 0 ? (items[i] ?? null) : null;
  };

  const onDragStart = (e: DragStartEvent) => setDragging(itemOf(String(e.active.id)));
  const onDragEnd = ({ active, over }: DragEndEvent) => {
    setDragging(null);
    const from = String(active.id);
    const to = over ? String(over.id) : null;
    const overSlot = to ? slots.indexOf(to) : -1;
    if (from.startsWith("palette:")) {
      const id = itemOf(from);
      if (!id) return;
      if (overSlot >= 0) setItems(insertItem(items, id, overSlot));
      else if (to === STRIP) setItems(insertItem(items, id, items.length));
      return;
    }
    const index = slots.indexOf(from);
    if (index < 0) return;
    if (overSlot >= 0) {
      if (overSlot !== index) setItems(moveItem(items, index, overSlot));
    } else if (to === PALETTE || to === null) {
      // Dragged out of the toolbar: removed (Safari).
      setItems(removeItemAt(items, index));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="customize-toolbar"
        data-scope="dialog"
        className="top-14 translate-y-0 sm:max-w-2xl"
      >
        <DialogHeader>
          <DialogTitle>Customize Toolbar</DialogTitle>
          <DialogDescription>
            Drag items into the toolbar, or out of it to remove them. Double-click an item to add it
            at the end.
          </DialogDescription>
        </DialogHeader>
        <DndContext
          sensors={sensors}
          collisionDetection={pointerWithin}
          onDragStart={onDragStart}
          onDragEnd={onDragEnd}
          onDragCancel={() => setDragging(null)}
        >
          <div className="flex min-w-0 flex-col gap-3">
            <Strip items={items} setItems={setItems} />
            <Palette
              items={paletteItems(items)}
              onAdd={(id) => setItems(insertItem(items, id, items.length))}
            />
            <div className="flex items-center gap-3">
              <span className="text-muted-foreground text-xs">…or the default set:</span>
              <div className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden opacity-70">
                {DEFAULT_TOOLBAR_ITEMS.map((id) => (
                  <Chip key={id} id={id} className="h-6 px-1.5 text-[10px] shadow-none" />
                ))}
              </div>
              <Button
                variant="outline"
                size="xs"
                onClick={() => setItems([...DEFAULT_TOOLBAR_ITEMS])}
              >
                Restore Default Set
              </Button>
            </div>
          </div>
          <DragOverlay dropAnimation={null}>{dragging && <Chip id={dragging} />}</DragOverlay>
        </DndContext>
        <DialogFooter className="items-center sm:justify-between">
          <label className="text-muted-foreground flex items-center gap-2 text-xs">
            Show
            <Select
              value={display}
              onValueChange={(v) => setPref("toolbarDisplay", v as ToolbarDisplay)}
            >
              <SelectTrigger
                size="sm"
                aria-label="Show"
                className="text-foreground h-7 w-36 rounded-md text-xs"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent data-scope="dialog">
                {TOOLBAR_DISPLAYS.map((d) => (
                  <SelectItem key={d} value={d} className="text-xs">
                    {TOOLBAR_DISPLAY_LABEL[d]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
