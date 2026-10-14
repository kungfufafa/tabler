import { Check, Plus, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useShallow } from "zustand/react/shallow";
import { ColorDot } from "@/components/ColorDot";
import { Kbd } from "@/components/Kbd";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { actions, useAction } from "@/features/palette/actions";
import { DRIVER_SHORT } from "@/app/connectionLabels";
import { comboFromEvent } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import {
  showWorkspace,
  toggleWorkspaceSwitcher,
  useRailOrder,
  useWorkspaceSwitcher,
  workspaceEntries,
} from "./workspaces";

function Switcher({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  // Rail order (the user may have dragged it).
  const order = useRailOrder();
  const byId = useConnectionsStore((s) => s.byId);
  const activeId = useConnectionsStore((s) => s.activeId);
  const launcher = useUiStore((s) => s.view === "launcher");
  const counts = useTabsStore(useShallow((s) => s.tabs.map((t) => t.connectionId)));
  const tabCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const id of counts) m.set(id, (m.get(id) ?? 0) + 1);
    return m;
  }, [counts]);
  const entries = useMemo(
    () => workspaceEntries(order, byId, tabCounts, query),
    [order, byId, tabCounts, query],
  );
  // The last row opens the connection manager ("add a workspace").
  const total = entries.length + 1;
  const [selected, setSelected] = useState(() =>
    Math.max(
      0,
      entries.findIndex((e) => e.connectionId === activeId),
    ),
  );
  const index = Math.min(selected, total - 1);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${index}"]`)
      ?.scrollIntoView?.({ block: "nearest" });
  }, [index]);

  const choose = (i: number) => {
    onClose();
    const entry = entries[i];
    if (entry) showWorkspace(entry.connectionId);
    else void actions.run("ui.launcher");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const combo = comboFromEvent(e.nativeEvent);
    // Global shortcuts are inert in dialogs: the opener closes it again here.
    if (combo === shortcutFor("workspace.show")) {
      e.preventDefault();
      onClose();
      return;
    }
    const digit = /^Mod\+([1-9])$/.exec(combo)?.[1];
    if (digit && Number(digit) <= entries.length) {
      e.preventDefault();
      choose(Number(digit) - 1);
      return;
    }
    if (e.key === "ArrowDown" || combo === "Ctrl+N") {
      e.preventDefault();
      setSelected((index + 1) % total);
    } else if (e.key === "ArrowUp" || combo === "Ctrl+P") {
      e.preventDefault();
      setSelected((index - 1 + total) % total);
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(index);
    }
  };

  return (
    <div className="flex min-h-0 flex-col" onKeyDown={onKeyDown}>
      <div className="border-border flex h-10 items-center gap-2.5 border-b px-3.5">
        <Search className="text-muted-foreground size-4 shrink-0" />
        <input
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSelected(0);
          }}
          placeholder="Switch to workspace…"
          aria-label="Search workspaces"
          aria-controls="workspace-list"
          aria-activedescendant={`workspace-option-${index}`}
          spellCheck={false}
          autoComplete="off"
          className="placeholder:text-muted-foreground/80 h-full min-w-0 flex-1 bg-transparent text-sm outline-none"
        />
      </div>
      <div
        ref={listRef}
        id="workspace-list"
        role="listbox"
        aria-label="Open workspaces"
        className="max-h-80 overflow-y-auto p-1.5"
      >
        {entries.length === 0 && (
          <div className="text-muted-foreground px-2 py-3 text-center text-xs">
            {order.length === 0 ? "No open workspaces." : "No workspace matches."}
          </div>
        )}
        {entries.map((e, i) => {
          const current = !launcher && e.connectionId === activeId;
          return (
            <div
              key={e.connectionId}
              id={`workspace-option-${i}`}
              role="option"
              aria-selected={i === index}
              data-index={i}
              data-connection-id={e.connectionId}
              onMouseMove={() => setSelected(i)}
              onClick={() => choose(i)}
              className={cn(
                "flex h-8 cursor-default items-center gap-2.5 rounded-md px-2 text-sm select-none",
                i === index && "bg-brand-soft",
              )}
            >
              <ColorDot color={e.color} size={9} />
              <span className="text-muted-foreground w-5 shrink-0 text-[10px] font-bold">
                {DRIVER_SHORT[e.driver]}
              </span>
              <span className="min-w-0 truncate font-medium">{e.name}</span>
              {e.database && (
                <span className="text-muted-foreground min-w-0 shrink-[3] truncate text-xs">
                  {e.database}
                </span>
              )}
              <span className="ml-auto flex shrink-0 items-center gap-2 pl-3">
                <span className="text-muted-foreground text-[11px] tabular-nums">
                  {e.tabCount === 1 ? "1 tab" : `${e.tabCount} tabs`}
                </span>
                {current && <Check className="text-brand size-3.5" aria-label="Current" />}
                {i < 9 && <Kbd combo={`Mod+${i + 1}`} className="bg-transparent" />}
              </span>
            </div>
          );
        })}
        <div
          id={`workspace-option-${entries.length}`}
          role="option"
          aria-selected={index === entries.length}
          data-index={entries.length}
          onMouseMove={() => setSelected(entries.length)}
          onClick={() => choose(entries.length)}
          className={cn(
            "text-muted-foreground flex h-8 cursor-default items-center gap-2.5 rounded-md px-2 text-sm select-none",
            index === entries.length && "bg-brand-soft text-foreground",
          )}
        >
          <Plus className="size-3.5" aria-hidden />
          Open Connection…
        </div>
      </div>
    </div>
  );
}

/**
 * Show Workspaces (TablePlus toolbar / ⇧⌘\): the open workspaces with their connection colour,
 * database and tab count; ↑↓ and ↩ (or ⌘1…9, or a click) switch, the last row opens the
 * connection manager to add one. Mounted once by the app shell; registers `workspace.show`.
 */
export function WorkspaceSwitcherHost() {
  const open = useWorkspaceSwitcher((s) => s.open);
  useAction("workspace.show", toggleWorkspaceSwitcher);
  const close = () => useWorkspaceSwitcher.setState({ open: false });
  return (
    <Dialog open={open} onOpenChange={(o) => useWorkspaceSwitcher.setState({ open: o })}>
      <DialogContent
        showCloseButton={false}
        data-scope="dialog"
        className="bg-popover ring-foreground/10 top-[14vh] max-w-[calc(100%-2rem)] translate-y-0 gap-0 overflow-hidden rounded-xl p-0 shadow-2xl sm:max-w-[480px] dark:ring-white/10"
      >
        <DialogTitle className="sr-only">Show Workspaces</DialogTitle>
        <DialogDescription className="sr-only">
          Switch to an open workspace or open another connection.
        </DialogDescription>
        {open && <Switcher onClose={close} />}
      </DialogContent>
    </Dialog>
  );
}
