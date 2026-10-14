import {
  ContextMenuCheckboxItem,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenuCheckboxItem,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu";
import { formatCombo } from "@/lib/keys/combo";
import type { MenuEntry } from "./menu";

/** Renders a menu model inside a `ContextMenuContent`. */
export function ContextMenuEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return entries.map((e) => {
    switch (e.kind) {
      case "separator":
        return <ContextMenuSeparator key={e.id} />;
      case "label":
        return (
          <ContextMenuLabel key={e.id} className="text-muted-foreground text-[11px]">
            {e.label}
          </ContextMenuLabel>
        );
      case "check":
        return (
          <ContextMenuCheckboxItem
            key={e.id}
            checked={e.checked}
            onCheckedChange={(v) => e.onChange(v === true)}
          >
            {e.label}
          </ContextMenuCheckboxItem>
        );
      case "radio":
        return (
          <ContextMenuRadioGroup key={e.id} value={e.value} onValueChange={e.onChange}>
            {e.options.map((o) => (
              <ContextMenuRadioItem key={o.value} value={o.value}>
                {o.label}
              </ContextMenuRadioItem>
            ))}
          </ContextMenuRadioGroup>
        );
      case "sub":
        return (
          <ContextMenuSub key={e.id}>
            <ContextMenuSubTrigger>
              {e.icon && <e.icon />} {e.label}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuEntries entries={e.items} />
            </ContextMenuSubContent>
          </ContextMenuSub>
        );
      case "item":
        return (
          <ContextMenuItem
            key={e.id}
            disabled={e.disabled}
            variant={e.destructive ? "destructive" : "default"}
            onSelect={e.onSelect}
          >
            {e.icon && <e.icon />} {e.label}
            {e.shortcut && <ContextMenuShortcut>{formatCombo(e.shortcut)}</ContextMenuShortcut>}
          </ContextMenuItem>
        );
    }
  });
}

/** Renders a menu model inside a `DropdownMenuContent`. */
export function DropdownMenuEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return entries.map((e) => {
    switch (e.kind) {
      case "separator":
        return <DropdownMenuSeparator key={e.id} />;
      case "label":
        return (
          <DropdownMenuLabel key={e.id} className="text-muted-foreground text-[11px]">
            {e.label}
          </DropdownMenuLabel>
        );
      case "check":
        return (
          <DropdownMenuCheckboxItem
            key={e.id}
            checked={e.checked}
            onCheckedChange={(v) => e.onChange(v === true)}
          >
            {e.label}
          </DropdownMenuCheckboxItem>
        );
      case "radio":
        return (
          <DropdownMenuRadioGroup key={e.id} value={e.value} onValueChange={e.onChange}>
            {e.options.map((o) => (
              <DropdownMenuRadioItem key={o.value} value={o.value}>
                {o.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        );
      case "sub":
        return (
          <DropdownMenuSub key={e.id}>
            <DropdownMenuSubTrigger>
              {e.icon && <e.icon />} {e.label}
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              <DropdownMenuEntries entries={e.items} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        );
      case "item":
        return (
          <DropdownMenuItem
            key={e.id}
            disabled={e.disabled}
            variant={e.destructive ? "destructive" : "default"}
            onSelect={e.onSelect}
          >
            {e.icon && <e.icon />} {e.label}
            {e.shortcut && <DropdownMenuShortcut>{formatCombo(e.shortcut)}</DropdownMenuShortcut>}
          </DropdownMenuItem>
        );
    }
  });
}
