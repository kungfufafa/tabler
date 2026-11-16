import type { LucideIcon } from "lucide-react";
import {
  ContextMenuItem,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import {
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu";
import { formatCombo, isMac } from "@/lib/keys/combo";

/**
 * One menu described once, drawn as a context menu (right-click) or a dropdown ("…" button):
 * the welcome screen shows the same items both ways.
 */
export type MenuEntry =
  | {
      kind: "item";
      label: string;
      icon?: LucideIcon;
      run: () => void;
      disabled?: boolean;
      destructive?: boolean;
      /** Nesting level (a group tree drawn in a menu): indents the item. */
      indent?: number;
      /**
       * The key that does the same from the list ("Mod+C"), shown at the right like a native
       * menu's accelerator. It only labels the item: the list handles the key itself.
       */
      shortcut?: string;
    }
  | { kind: "sub"; label: string; icon?: LucideIcon; items: MenuEntry[]; disabled?: boolean }
  | {
      kind: "radio";
      value: string;
      options: ReadonlyArray<{ value: string; label: string }>;
      onChange: (value: string) => void;
    }
  | { kind: "separator" };

/** Leading, trailing and doubled separators left out (items may be conditional). */
function tidy(entries: readonly MenuEntry[]): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const e of entries) {
    if (e.kind === "separator" && (out.length === 0 || out.at(-1)?.kind === "separator")) continue;
    out.push(e);
  }
  while (out.at(-1)?.kind === "separator") out.pop();
  return out;
}

/** `aria-keyshortcuts` form of a combo ("Mod+C" → "Control+C", "Meta+C" on macOS). */
const ariaKeys = (combo: string | undefined) =>
  combo
    ?.split("+")
    .map((p) => (p === "Mod" ? (isMac ? "Meta" : "Control") : p === "Ctrl" ? "Control" : p))
    .join("+");

/** Spelled-out keys ("Ctrl+Shift+C") read badly with the menus' glyph spacing. */
const shortcutClass = isMac ? undefined : "tracking-normal";

/** The item's left padding for its nesting level (the menus' items start at 8px). */
const indentStyle = (indent: number | undefined) =>
  indent ? { paddingLeft: 8 + indent * 14 } : undefined;

export function ContextMenuEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return tidy(entries).map((e, i) => {
    switch (e.kind) {
      case "separator":
        return <ContextMenuSeparator key={`sep-${i}`} />;
      case "radio":
        return (
          <ContextMenuRadioGroup key={`radio-${i}`} value={e.value} onValueChange={e.onChange}>
            {e.options.map((o) => (
              <ContextMenuRadioItem key={o.value} value={o.value}>
                {o.label}
              </ContextMenuRadioItem>
            ))}
          </ContextMenuRadioGroup>
        );
      case "sub":
        return (
          <ContextMenuSub key={`sub-${i}-${e.label}`}>
            <ContextMenuSubTrigger disabled={e.disabled}>
              {e.icon && <e.icon />} {e.label}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent className="min-w-44">
              <ContextMenuEntries entries={e.items} />
            </ContextMenuSubContent>
          </ContextMenuSub>
        );
      case "item":
        return (
          <ContextMenuItem
            key={`item-${i}-${e.label}`}
            variant={e.destructive ? "destructive" : "default"}
            disabled={e.disabled}
            onSelect={e.run}
            style={indentStyle(e.indent)}
            aria-keyshortcuts={ariaKeys(e.shortcut)}
          >
            {e.icon && <e.icon />} {e.label}
            {/* Announced through aria-keyshortcuts; hidden so the item's name stays its label. */}
            {e.shortcut && (
              <ContextMenuShortcut aria-hidden className={shortcutClass}>
                {formatCombo(e.shortcut)}
              </ContextMenuShortcut>
            )}
          </ContextMenuItem>
        );
    }
  });
}

export function DropdownMenuEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return tidy(entries).map((e, i) => {
    switch (e.kind) {
      case "separator":
        return <DropdownMenuSeparator key={`sep-${i}`} />;
      case "radio":
        return (
          <DropdownMenuRadioGroup key={`radio-${i}`} value={e.value} onValueChange={e.onChange}>
            {e.options.map((o) => (
              <DropdownMenuRadioItem key={o.value} value={o.value}>
                {o.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        );
      case "sub":
        return (
          <DropdownMenuSub key={`sub-${i}-${e.label}`}>
            <DropdownMenuSubTrigger disabled={e.disabled}>
              {e.icon && <e.icon />} {e.label}
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent className="min-w-44">
              <DropdownMenuEntries entries={e.items} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        );
      case "item":
        return (
          <DropdownMenuItem
            key={`item-${i}-${e.label}`}
            variant={e.destructive ? "destructive" : "default"}
            disabled={e.disabled}
            onSelect={e.run}
            style={indentStyle(e.indent)}
            aria-keyshortcuts={ariaKeys(e.shortcut)}
          >
            {e.icon && <e.icon />} {e.label}
            {/* Announced through aria-keyshortcuts; hidden so the item's name stays its label. */}
            {e.shortcut && (
              <DropdownMenuShortcut aria-hidden className={shortcutClass}>
                {formatCombo(e.shortcut)}
              </DropdownMenuShortcut>
            )}
          </DropdownMenuItem>
        );
    }
  });
}
