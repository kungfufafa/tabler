import { Command as CommandPrimitive } from "cmdk";
import {
  Check,
  Database,
  Minus,
  Plus,
  FileCode2,
  Layers,
  LayoutGrid,
  LoaderCircle,
  Search,
  Table2,
  TableProperties,
} from "lucide-react";
import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { toast } from "sonner";
import { ColorDot } from "@/components/ColorDot";
import { Kbd } from "@/components/Kbd";
import { TableIcon } from "@/components/TypeIcon";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { openCreateDatabase, openDropDatabase } from "@/features/admin/store";
import { comboFromEvent, isMac } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import type { ActionId } from "./actions";
import { usePaletteCommands, type PaletteCommands } from "./commands";
import type { MatchRange } from "./fuzzy";
import {
  COMMAND_PREFIX,
  completionOf,
  flattenSections,
  type PaletteIcon,
  type PaletteItem,
  type PaletteMenuEntry,
  type PaletteSection,
  type RankedItem,
} from "./items";
import { focusTabSurface } from "./focus";
import { recordMru } from "./mru";
import { usePaletteStore, type PaletteMode } from "./store";
import { usePaletteItems } from "./usePaletteItems";

type PaletteRun = () => unknown;

function reportFailure(e: unknown) {
  toast.error("Command failed", { description: e instanceof Error ? e.message : String(e) });
}

/** Runs a chosen command now. Failures surface as a toast; a newly activated tab gets focus. */
function execute(fn: PaletteRun) {
  const tabBefore = useTabsStore.getState().activeId;
  const viewBefore = useUiStore.getState().view;
  let result: unknown;
  try {
    result = fn();
  } catch (e) {
    reportFailure(e);
    return;
  }
  const tabAfter = useTabsStore.getState().activeId;
  const shown = useUiStore.getState().view === "workspace";
  if (tabAfter !== null && shown && (tabAfter !== tabBefore || viewBefore !== "workspace"))
    focusTabSurface();
  Promise.resolve(result).catch(reportFailure);
}

/** The palette each opener shows. */
const OPENER_MODES: Partial<Record<ActionId, PaletteMode>> = {
  "palette.open": "all",
  "palette.databases": "databases",
  "palette.connections": "connections",
};

/**
 * ⌘P / ⌘K / ⇧⌘K inside the palette. Global shortcuts are suppressed while a dialog has focus, so
 * the palette handles its openers itself: the combo of the open mode closes it, another one
 * switches the mode.
 */
const TOGGLE_COMBOS: ReadonlyMap<string, PaletteMode> = new Map(
  KEYMAP.flatMap((d) => {
    const mode = OPENER_MODES[d.id];
    return mode ? [[d.keys, mode] as const] : [];
  }),
);

const MODE_TEXT: Record<
  PaletteMode,
  { title: string; description: string; placeholder: string; verb: string; empty: string }
> = {
  all: {
    title: "Open anything",
    description:
      "Search tables, views, schemas, databases, open tabs, connections and commands. Type > to list every command.",
    placeholder: "Search tables, views, databases, connections and commands…",
    verb: "Open",
    empty: "Nothing to show",
  },
  databases: {
    title: "Switch database",
    description: "Switch the active connection to another database or schema.",
    placeholder: "Switch database…",
    verb: "Switch",
    empty: "No other databases on this connection",
  },
  connections: {
    title: "Switch connection",
    description: "Switch to an open connection or connect a saved one.",
    placeholder: "Switch connection…",
    verb: "Switch",
    empty: "No saved connections",
  },
  objects: {
    title: "Search in Database",
    description:
      "Search every table, view and function of the database, in all of its schemas, loaded or not.",
    placeholder: "Search in database…",
    verb: "Open",
    empty: "Nothing in this database",
  },
};

const TAB_ICONS = {
  table: Table2,
  structure: TableProperties,
  query: FileCode2,
  tool: LayoutGrid,
} as const;

/** Open Anything (⌘P), Switch database (⌘K), Switch connection (⇧⌘K). */
export function CommandPalette() {
  const open = usePaletteStore((s) => s.open);
  const mode = usePaletteStore((s) => s.mode);
  const session = usePaletteStore((s) => s.session);
  const initialQuery = usePaletteStore((s) => s.initialQuery);
  const setOpen = usePaletteStore((s) => s.setOpen);
  const toggle = usePaletteStore((s) => s.toggle);
  const commands = usePaletteCommands();
  const pending = useRef<PaletteRun | null>(null);
  const restoreFocus = useRef<HTMLElement | null>(null);

  // Remember what had focus (grid, editor, sidebar…) synchronously at the moment of opening,
  // before the dialog's input steals it.
  useEffect(
    () =>
      usePaletteStore.subscribe((s, prev) => {
        if (!s.open || prev.open) return;
        const fn = pending.current;
        if (fn) {
          // Reopened (⌘P…) before the previous choice ran, i.e. during the close animation, when
          // the dialog never unmounts and its close-focus callback never fires. Run the choice now,
          // from where focus was, rather than behind the reopened palette or not at all.
          pending.current = null;
          const target = restoreFocus.current;
          if (target?.isConnected) target.focus({ preventScroll: true });
          execute(fn);
        }
        const el = document.activeElement;
        // Reopened during the close animation: keep the original target.
        if (el instanceof HTMLElement && el.closest("[cmdk-root]")) return;
        restoreFocus.current = el instanceof HTMLElement && el !== document.body ? el : null;
      }),
    [],
  );

  /** Runs the chosen command once focus is back where it was before the palette opened. */
  const flush = useCallback(() => {
    const fn = pending.current;
    pending.current = null;
    if (fn) window.setTimeout(() => execute(fn), 0);
  }, []);

  const runAfterClose = useCallback(
    (fn: PaletteRun) => {
      pending.current = fn;
      setOpen(false);
      // Safety net should the close-focus callback not fire (e.g. the shell unmounts).
      window.setTimeout(() => {
        if (pending.current === fn) flush();
      }, 500);
    },
    [flush, setOpen],
  );

  const toggleMode = useCallback(
    (next: PaletteMode) => {
      // Same fallback as the global ⌘K: without a connection there are no databases to list.
      const connected = useConnectionsStore.getState().activeId !== null;
      toggle(next === "databases" && !connected ? "connections" : next);
    },
    [toggle],
  );

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent
        showCloseButton={false}
        onCloseAutoFocus={(e) => {
          // A modal Radix dialog returns focus to its trigger, and the palette has none: put focus
          // back where it was ourselves, then run the chosen command from there.
          e.preventDefault();
          // A late callback from a previous closing while the palette is open again: the reopened
          // palette owns focus, and the choice already ran when it reopened.
          if (usePaletteStore.getState().open) return;
          const el = restoreFocus.current;
          restoreFocus.current = null;
          if (el?.isConnected) el.focus({ preventScroll: true });
          // The grid that had focus went away (its tab was switched meanwhile): use the new one.
          else if (el) focusTabSurface();
          flush();
        }}
        className="bg-popover ring-foreground/10 top-[14vh] max-w-[calc(100%-2rem)] translate-y-0 gap-0 overflow-hidden rounded-xl p-0 shadow-2xl sm:max-w-[620px] dark:ring-white/10"
      >
        <DialogTitle className="sr-only">{MODE_TEXT[mode].title}</DialogTitle>
        <DialogDescription className="sr-only">{MODE_TEXT[mode].description}</DialogDescription>
        <PaletteBody
          key={session}
          mode={mode}
          initialQuery={initialQuery}
          commands={commands}
          onRun={runAfterClose}
          onToggle={toggleMode}
        />
      </DialogContent>
    </Dialog>
  );
}

function PaletteBody({
  mode,
  initialQuery = "",
  commands,
  onRun,
  onToggle,
}: {
  mode: PaletteMode;
  /** What the query starts as (Search in Database… hands over the sidebar's search). */
  initialQuery?: string;
  commands: PaletteCommands;
  onRun: (fn: PaletteRun) => void;
  onToggle: (mode: PaletteMode) => void;
}) {
  // Not deferred on purpose: ↩ right after typing must act on the results for what was typed.
  // Ranking is cheap (a few ms for thousands of tables) and at most TABLE_LIMIT rows render.
  const [query, setQuery] = useState(initialQuery);
  const data = usePaletteItems(query, commands, mode);
  const text = MODE_TEXT[mode];
  const flat = useMemo(() => flattenSections(data.sections), [data.sections]);

  // Selection belongs to a query: a new query selects its best match.
  const [selection, setSelection] = useState<{ query: string; key: string } | null>(null);
  const selectedKey =
    selection && selection.query === query && flat.some((i) => i.key === selection.key)
      ? selection.key
      : (flat[0]?.key ?? "");
  const selected = flat.find((i) => i.key === selectedKey) ?? null;
  const modClick = useRef(false);
  // Every list (TablePlus): a click only selects a row, a double-click or ↩ opens it, ⌘-click
  // opens it in a new tab at once. A stray click while moving the pointer opens nothing.
  const pointerSelect = useRef(false);
  const listRef = useRef<HTMLDivElement>(null);
  const objects = (mode === "all" || mode === "objects") && !data.commandMode;
  // Inline completion (TablePlus): the rest of the selected object's name after what was typed.
  // (Not for a command listed after the objects: Tab would put its title in the query.)
  const ghost =
    objects && selected && selected.group !== "actions" ? completionOf(query, selected.label) : "";

  // A new query selects its best match, the first row. cmdk does not scroll to it because the
  // selected value is controlled (it never sees the selection change), so reset the scroll here.
  useLayoutEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
  }, [query]);

  const choose = (item: PaletteItem, alt: boolean) => {
    // Keys pressed during the close animation still reach the (fading) input; ignore them.
    if (!usePaletteStore.getState().open) return;
    // "Load all schemas": the list grows in place.
    if (item.keepOpen) {
      execute(item.run);
      return;
    }
    if (!item.noRecent) recordMru(item.key);
    onRun(alt && item.runAlt ? item.runAlt : item.run);
  };

  /** A row's right-click entry: most close the palette first, Copy Name keeps it open. */
  const runMenuEntry = (entry: PaletteMenuEntry) => {
    if (!usePaletteStore.getState().open) return;
    if (entry.keepOpen) execute(entry.run);
    else onRun(entry.run);
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    modClick.current = false;
    pointerSelect.current = false;
    if (e.nativeEvent.isComposing) return;
    const combo = comboFromEvent(e.nativeEvent);
    const toggleTo = TOGGLE_COMBOS.get(combo);
    if (toggleTo) {
      // Toggle rather than close: while the dialog is still animating out, ⌘P reopens it.
      e.preventDefault();
      onToggle(toggleTo);
    } else if (combo === "Mod+Enter" && selected) {
      e.preventDefault();
      choose(selected, true);
    } else if (ghost && selected && (combo === "Tab" || (combo === "Right" && caretAtEnd(e)))) {
      // Accept the completion.
      e.preventDefault();
      setQuery(selected.label);
    }
  };

  return (
    <CommandPrimitive
      label={text.title}
      shouldFilter={false}
      loop
      value={selectedKey}
      onValueChange={(key) => setSelection({ query, key })}
      onKeyDown={onKeyDown}
      className="flex min-h-0 flex-col"
    >
      <div className="border-border flex h-11 items-center gap-2.5 border-b px-3.5">
        <Search className="text-muted-foreground size-4 shrink-0" />
        <div className="relative h-full min-w-0 flex-1">
          {ghost && (
            <div
              aria-hidden
              data-testid="palette-completion"
              className="pointer-events-none absolute inset-0 flex items-center overflow-hidden text-base whitespace-pre"
            >
              <span className="invisible">{query}</span>
              <span className="text-muted-foreground/60">{ghost}</span>
            </div>
          )}
          <CommandPrimitive.Input
            autoFocus
            value={query}
            onValueChange={setQuery}
            placeholder={text.placeholder}
            className="placeholder:text-muted-foreground/80 relative h-full w-full bg-transparent text-base outline-none"
          />
        </div>
        {data.connection && (
          <span className="text-muted-foreground flex max-w-[40%] shrink-0 items-center gap-1.5 text-[11px]">
            <ColorDot color={data.connection.color} size={6} />
            <span className="truncate">
              {data.connection.name}
              {objects && data.schema && <span className="opacity-70"> · {data.schema}</span>}
            </span>
          </span>
        )}
      </div>

      <CommandPrimitive.List
        ref={listRef}
        className={cn(
          "max-h-[min(440px,58vh)] scroll-py-1 overflow-x-hidden overflow-y-auto overscroll-contain px-1.5 pb-1.5",
          "**:[[cmdk-group-heading]]:text-muted-foreground **:[[cmdk-group-heading]]:flex **:[[cmdk-group-heading]]:h-7 **:[[cmdk-group-heading]]:items-end **:[[cmdk-group-heading]]:px-2 **:[[cmdk-group-heading]]:pb-1 **:[[cmdk-group-heading]]:text-[11px] **:[[cmdk-group-heading]]:font-medium",
        )}
      >
        {data.sections.length === 0 && (
          <div className="text-muted-foreground py-10 text-center text-xs">
            {query.trim() ? `No results for “${query.trim()}”` : text.empty}
          </div>
        )}
        {data.sections.map((section) => (
          <Section
            key={section.id}
            section={section}
            onChoose={choose}
            modClick={modClick}
            pointerSelect={pointerSelect}
            onMenu={(key) => setSelection({ query, key })}
            onMenuEntry={runMenuEntry}
          />
        ))}
      </CommandPrimitive.List>

      <Footer
        selected={selected}
        verb={text.verb}
        hint={objects && mode === "all"}
        databaseActions={mode === "databases"}
        onRun={onRun}
      />
    </CommandPrimitive>
  );
}

function caretAtEnd(e: ReactKeyboardEvent): boolean {
  const input = e.target;
  return (
    input instanceof HTMLInputElement &&
    input.selectionStart === input.value.length &&
    input.selectionEnd === input.value.length
  );
}

/** Stands in for items that are still loading (tables, databases…). */
function LoadingRow({ what }: { what: string }) {
  return (
    <div className="text-muted-foreground flex h-7 items-center gap-2 px-2 text-xs">
      <LoaderCircle className="size-3.5 animate-spin" />
      Loading {what}…
    </div>
  );
}

/** A headed group (⌘K, ⇧⌘K), or Open Anything's flat list without a heading. */
function Section({
  section,
  onChoose,
  modClick,
  pointerSelect,
  onMenu,
  onMenuEntry,
}: {
  section: PaletteSection;
  onChoose: (item: PaletteItem, alt: boolean) => void;
  modClick: RefObject<boolean>;
  /** A click only selects (cmdk already did on hover), a double-click chooses; ⌘-click chooses. */
  pointerSelect: RefObject<boolean>;
  /** A row's context menu opens or a click lands on it: select that row. */
  onMenu: (key: string) => void;
  onMenuEntry: (entry: PaletteMenuEntry) => void;
}) {
  const body = (
    <>
      {section.items.map((ranked) => (
        <PaletteRow
          key={ranked.item.key}
          ranked={ranked}
          showKind={section.showKind ?? false}
          onPointerDown={(e) => {
            modClick.current = isMac ? e.metaKey : e.ctrlKey;
            pointerSelect.current = true;
          }}
          onSelect={() => {
            const alt = modClick.current;
            const byPointer = pointerSelect.current;
            modClick.current = false;
            pointerSelect.current = false;
            if (byPointer && !alt) {
              // A plain click: the row is selected, nothing opens (that is ↩ or a double-click).
              onMenu(ranked.item.key);
              return;
            }
            onChoose(ranked.item, alt);
          }}
          onDoubleClick={() => onChoose(ranked.item, false)}
          onMenu={() => onMenu(ranked.item.key)}
          onMenuEntry={onMenuEntry}
        />
      ))}
      {section.loading !== undefined && <LoadingRow what={section.loading} />}
      {section.more > 0 && (
        <div className="text-muted-foreground px-2 py-1 text-[11px]">
          …{section.more.toLocaleString()} more — keep typing to narrow down
        </div>
      )}
    </>
  );
  if (section.heading === undefined) return <div className="pt-1.5">{body}</div>;
  return <CommandPrimitive.Group heading={section.heading}>{body}</CommandPrimitive.Group>;
}

function PaletteRow({
  ranked,
  showKind,
  onSelect,
  onPointerDown,
  onDoubleClick,
  onMenu,
  onMenuEntry,
}: {
  ranked: RankedItem;
  /** Open Anything's objects: the kind on the right instead of the detail after the label. */
  showKind: boolean;
  onSelect: () => void;
  onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onDoubleClick?: () => void;
  onMenu: () => void;
  onMenuEntry: (entry: PaletteMenuEntry) => void;
}) {
  const row = (
    <PaletteItemRow
      ranked={ranked}
      showKind={showKind}
      onSelect={onSelect}
      onPointerDown={onPointerDown}
      onDoubleClick={onDoubleClick}
    />
  );
  const menu = ranked.item.menu;
  if (!menu || menu.length === 0) return row;
  return (
    <ContextMenu onOpenChange={(open) => open && onMenu()}>
      <ContextMenuTrigger asChild>{row}</ContextMenuTrigger>
      <ContextMenuContent className="min-w-48">
        {menu.map((entry) => (
          <Fragment key={entry.label}>
            {entry.separated && <ContextMenuSeparator />}
            <ContextMenuItem
              variant={entry.destructive ? "destructive" : "default"}
              disabled={entry.disabled}
              onSelect={() => onMenuEntry(entry)}
            >
              {entry.label}
            </ContextMenuItem>
          </Fragment>
        ))}
      </ContextMenuContent>
    </ContextMenu>
  );
}

function PaletteItemRow({
  ranked: { item, ranges },
  showKind,
  onSelect,
  onPointerDown,
  onDoubleClick,
  ...rest
}: {
  ranked: RankedItem;
  showKind: boolean;
  onSelect: () => void;
  onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onDoubleClick?: () => void;
}) {
  return (
    <CommandPrimitive.Item
      // ContextMenuTrigger (asChild) hands its handlers and data attributes down.
      {...rest}
      value={item.key}
      onSelect={onSelect}
      // A ContextMenuTrigger parent composes its own pointer handler into this one.
      onPointerDown={onPointerDown}
      onDoubleClick={onDoubleClick}
      className="data-[selected=true]:bg-brand-soft relative flex h-7 cursor-default items-center gap-2 rounded-md px-2 text-sm outline-none select-none"
    >
      <ItemIcon icon={item.icon} />
      <span className="min-w-0 truncate">
        <Highlight text={item.label} ranges={ranges} />
      </span>
      {item.detail && !showKind && (
        <span className="text-muted-foreground min-w-0 shrink-[3] truncate text-xs">
          {item.detail}
        </span>
      )}
      {item.dirty && (
        <span
          className="bg-warning size-1.5 shrink-0 rounded-full"
          title="Unsaved changes"
          aria-label="Unsaved changes"
        />
      )}
      <span className="ml-auto flex shrink-0 items-center gap-2 pl-3">
        {item.meta && (
          <span className="text-muted-foreground font-mono text-[11px] tabular-nums">
            {item.meta}
          </span>
        )}
        {item.current && <Check className="text-brand size-3.5" aria-label="Current" />}
        {item.combo && <Kbd combo={item.combo} />}
        {showKind && item.kind && (
          <span className="text-muted-foreground text-[11px]">{item.kind}</span>
        )}
      </span>
    </CommandPrimitive.Item>
  );
}

function ItemIcon({ icon }: { icon: PaletteIcon }) {
  switch (icon.type) {
    case "table":
      return <TableIcon kind={icon.kind} />;
    case "tab": {
      const Icon = TAB_ICONS[icon.kind];
      return <Icon className="text-muted-foreground size-3.5 shrink-0" />;
    }
    case "connection":
      return <Database className="size-3.5 shrink-0" style={{ color: accentColor(icon.color) }} />;
    case "database":
      return <Database className="text-muted-foreground size-3.5 shrink-0" />;
    case "schema":
      return <Layers className="text-muted-foreground size-3.5 shrink-0" />;
    case "action": {
      const Icon = icon.icon;
      return <Icon className="text-muted-foreground size-3.5 shrink-0" />;
    }
  }
}

/**
 * Matched characters: semibold, tinted towards the accent. Raw `text-brand` is too faint for 13px
 * text on the popover / selected row in dark mode and with light accents (down to ~2.3:1); half
 * brand, half foreground keeps every accent above 5:1 in both themes.
 */
const MATCH_CLASS =
  "bg-transparent font-semibold text-[color-mix(in_oklab,var(--brand)_50%,var(--foreground))]";

/** Renders `text` with the fuzzy-matched ranges emphasised. */
function Highlight({ text, ranges }: { text: string; ranges: readonly MatchRange[] }) {
  if (ranges.length === 0) return <>{text}</>;
  const parts: ReactNode[] = [];
  let pos = 0;
  ranges.forEach(([start, end]) => {
    if (start > pos) parts.push(text.slice(pos, start));
    parts.push(
      <mark key={start} className={MATCH_CLASS}>
        {text.slice(start, end)}
      </mark>,
    );
    pos = end;
  });
  if (pos < text.length) parts.push(text.slice(pos));
  return <>{parts}</>;
}

function Footer({
  selected,
  verb,
  hint,
  databaseActions,
  onRun,
}: {
  selected: PaletteItem | null;
  verb: string;
  /** Shows "Type > for all commands" (Open Anything's object list). */
  hint: boolean;
  /** ⌘K: the database list's + / − buttons. */
  databaseActions: boolean;
  onRun: (fn: PaletteRun) => void;
}) {
  return (
    <div className="border-border bg-surface-2 text-muted-foreground flex h-7 shrink-0 items-center gap-3 border-t px-3 text-[11px]">
      {databaseActions && <DatabaseButtons selected={selected} onRun={onRun} />}
      <span className="flex items-center gap-1">
        <Kbd combo="Up" />
        <Kbd combo="Down" />
        Navigate
      </span>
      <span className="flex items-center gap-1">
        <Kbd combo="Enter" />
        {selected?.verb ?? verb}
      </span>
      {selected?.runAlt && (
        <span className="flex items-center gap-1">
          <Kbd combo="Mod+Enter" />
          {selected.altLabel ?? "Alternate"}
        </span>
      )}
      {hint && (
        <span className="ml-auto" data-testid="palette-command-hint">
          Type <kbd className="font-mono">{COMMAND_PREFIX}</kbd> for all commands
        </span>
      )}
      <span className={cn("flex items-center gap-1", !hint && "ml-auto")}>
        <Kbd combo="Escape" />
        Close
      </span>
    </div>
  );
}

/**
 * ⌘K's "+" (New Database…) and "−" (Drop the selected database), like the buttons under
 * TablePlus's database list. PostgreSQL and MySQL only; the current database cannot be dropped.
 */
function DatabaseButtons({
  selected,
  onRun,
}: {
  selected: PaletteItem | null;
  onRun: (fn: PaletteRun) => void;
}) {
  const active = useActiveConnection();
  const driver = active?.info.driver;
  if (!active || (driver !== "postgres" && driver !== "mysql")) return null;
  const connectionId = active.info.connectionId;
  const database =
    selected?.group === "databases" && selected.icon.type === "database" ? selected : null;
  const dropTitle = !database
    ? "Drop Database… (select a database)"
    : database.current
      ? "The current database cannot be dropped"
      : `Drop ${database.label}…`;
  const button =
    "hover:bg-muted hover:text-foreground flex size-5 items-center justify-center rounded disabled:pointer-events-none disabled:opacity-40";
  return (
    <span className="-ml-1.5 flex items-center gap-0.5">
      <button
        type="button"
        aria-label="New Database…"
        title="New Database…"
        className={button}
        onClick={() => onRun(() => openCreateDatabase(connectionId))}
      >
        <Plus className="size-3.5" />
      </button>
      <button
        type="button"
        aria-label="Drop Database…"
        title={dropTitle}
        className={button}
        disabled={!database || database.current === true}
        onClick={() => database && onRun(() => openDropDatabase(connectionId, database.label))}
      >
        <Minus className="size-3.5" />
      </button>
    </span>
  );
}
