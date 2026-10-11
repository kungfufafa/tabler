import { Command as CommandPrimitive } from "cmdk";
import {
  Check,
  Database,
  FileCode2,
  LoaderCircle,
  Search,
  Table2,
  TableProperties,
} from "lucide-react";
import {
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
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { comboFromEvent, isMac } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { accentColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { usePaletteCommands, type PaletteCommands } from "./commands";
import type { MatchRange } from "./fuzzy";
import {
  flattenSections,
  type PaletteIcon,
  type PaletteItem,
  type PaletteSection,
  type RankedItem,
} from "./items";
import { focusTabSurface } from "./focus";
import { recordMru } from "./mru";
import { usePaletteStore } from "./store";
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

/**
 * ⌘K / ⌘P inside the palette. Global shortcuts are suppressed while a dialog has focus, so the
 * palette handles its own toggle combos.
 */
const TOGGLE_COMBOS = new Set(KEYMAP.filter((d) => d.id === "palette.open").map((d) => d.keys));
const TAB_ICONS = { table: Table2, structure: TableProperties, query: FileCode2 } as const;

/** Command palette / quick open (⌘K, ⌘P). */
export function CommandPalette() {
  const open = usePaletteStore((s) => s.open);
  const session = usePaletteStore((s) => s.session);
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
          // Reopened (⌘K) before the previous choice ran, i.e. during the close animation, when
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
        <DialogTitle className="sr-only">Command palette</DialogTitle>
        <DialogDescription className="sr-only">
          Search tables, open tabs, connections and actions.
        </DialogDescription>
        <PaletteBody key={session} commands={commands} onRun={runAfterClose} onToggle={toggle} />
      </DialogContent>
    </Dialog>
  );
}

function PaletteBody({
  commands,
  onRun,
  onToggle,
}: {
  commands: PaletteCommands;
  onRun: (fn: PaletteRun) => void;
  onToggle: () => void;
}) {
  // Not deferred on purpose: ↩ right after typing must act on the results for what was typed.
  // Ranking is cheap (a few ms for thousands of tables) and at most TABLE_LIMIT rows render.
  const [query, setQuery] = useState("");
  const data = usePaletteItems(query, commands);
  const flat = useMemo(() => flattenSections(data.sections), [data.sections]);

  // Selection belongs to a query: a new query selects its best match.
  const [selection, setSelection] = useState<{ query: string; key: string } | null>(null);
  const selectedKey =
    selection && selection.query === query && flat.some((i) => i.key === selection.key)
      ? selection.key
      : (flat[0]?.key ?? "");
  const selected = flat.find((i) => i.key === selectedKey) ?? null;
  const modClick = useRef(false);
  const listRef = useRef<HTMLDivElement>(null);

  // A new query selects its best match, the first row. cmdk does not scroll to it because the
  // selected value is controlled (it never sees the selection change), so reset the scroll here.
  useLayoutEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
  }, [query]);

  const choose = (item: PaletteItem, alt: boolean) => {
    // Keys pressed during the close animation still reach the (fading) input; ignore them.
    if (!usePaletteStore.getState().open) return;
    if (!item.noRecent) recordMru(item.key);
    onRun(alt && item.runAlt ? item.runAlt : item.run);
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    modClick.current = false;
    if (e.nativeEvent.isComposing) return;
    const combo = comboFromEvent(e.nativeEvent);
    if (TOGGLE_COMBOS.has(combo)) {
      // Toggle rather than close: while the dialog is still animating out, ⌘K reopens it.
      e.preventDefault();
      onToggle();
    } else if (combo === "Mod+Enter" && selected) {
      e.preventDefault();
      choose(selected, true);
    }
  };

  // While tables load, a placeholder takes the Tables group's place (after "Recent").
  const nodes: ReactNode[] = [];
  let loadingPlaced = !data.tablesLoading || data.sections.some((s) => s.id === "tables");
  for (const section of data.sections) {
    if (!loadingPlaced && section.id !== "recent") {
      nodes.push(<TablesLoading key="tables-loading" />);
      loadingPlaced = true;
    }
    nodes.push(
      <Section
        key={section.id}
        section={section}
        schema={data.schema}
        onChoose={choose}
        modClick={modClick}
      />,
    );
  }
  if (!loadingPlaced) nodes.push(<TablesLoading key="tables-loading" />);

  return (
    <CommandPrimitive
      label="Command palette"
      shouldFilter={false}
      loop
      value={selectedKey}
      onValueChange={(key) => setSelection({ query, key })}
      onKeyDown={onKeyDown}
      className="flex min-h-0 flex-col"
    >
      <div className="border-border flex h-11 items-center gap-2.5 border-b px-3.5">
        <Search className="text-muted-foreground size-4 shrink-0" />
        <CommandPrimitive.Input
          autoFocus
          value={query}
          onValueChange={setQuery}
          placeholder="Search tables, tabs, connections and actions…"
          className="placeholder:text-muted-foreground/80 h-full min-w-0 flex-1 bg-transparent text-base outline-none"
        />
        {data.connection && (
          <span className="text-muted-foreground flex max-w-[40%] shrink-0 items-center gap-1.5 text-[11px]">
            <ColorDot color={data.connection.color} size={6} />
            <span className="truncate">{data.connection.name}</span>
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
        {data.sections.length === 0 && !data.tablesLoading && (
          <div className="text-muted-foreground py-10 text-center text-xs">
            No results for “{query.trim()}”
          </div>
        )}
        {nodes}
      </CommandPrimitive.List>

      <Footer selected={selected} />
    </CommandPrimitive>
  );
}

function TablesLoading() {
  return (
    <CommandPrimitive.Group heading="Tables">
      <div className="text-muted-foreground flex h-7 items-center gap-2 px-2 text-xs">
        <LoaderCircle className="size-3.5 animate-spin" />
        Loading tables…
      </div>
    </CommandPrimitive.Group>
  );
}

function Section({
  section,
  schema,
  onChoose,
  modClick,
}: {
  section: PaletteSection;
  schema: string | null;
  onChoose: (item: PaletteItem, alt: boolean) => void;
  modClick: RefObject<boolean>;
}) {
  const heading: ReactNode =
    section.id === "tables" && schema ? (
      <>
        {section.heading}
        <span className="ml-1 font-normal opacity-70">· {schema}</span>
      </>
    ) : (
      section.heading
    );
  return (
    <CommandPrimitive.Group heading={heading}>
      {section.items.map((ranked) => (
        <PaletteRow
          key={ranked.item.key}
          ranked={ranked}
          onPointerDown={(e) => {
            modClick.current = isMac ? e.metaKey : e.ctrlKey;
          }}
          onSelect={() => {
            const alt = modClick.current;
            modClick.current = false;
            onChoose(ranked.item, alt);
          }}
        />
      ))}
      {section.more > 0 && (
        <div className="text-muted-foreground px-2 py-1 text-[11px]">
          …{section.more.toLocaleString()} more — keep typing to narrow down
        </div>
      )}
    </CommandPrimitive.Group>
  );
}

function PaletteRow({
  ranked: { item, ranges },
  onSelect,
  onPointerDown,
}: {
  ranked: RankedItem;
  onSelect: () => void;
  onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
}) {
  return (
    <CommandPrimitive.Item
      value={item.key}
      onSelect={onSelect}
      onPointerDown={onPointerDown}
      className="data-[selected=true]:bg-brand-soft relative flex h-7 cursor-default items-center gap-2 rounded-md px-2 text-sm outline-none select-none"
    >
      <ItemIcon icon={item.icon} />
      <span className="min-w-0 truncate">
        <Highlight text={item.label} ranges={ranges} />
      </span>
      {item.detail && (
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

function Footer({ selected }: { selected: PaletteItem | null }) {
  return (
    <div className="border-border bg-surface-2 text-muted-foreground flex h-7 shrink-0 items-center gap-3 border-t px-3 text-[11px]">
      <span className="flex items-center gap-1">
        <Kbd combo="Up" />
        <Kbd combo="Down" />
        Navigate
      </span>
      <span className="flex items-center gap-1">
        <Kbd combo="Enter" />
        {selected?.verb ?? "Open"}
      </span>
      {selected?.runAlt && (
        <span className="flex items-center gap-1">
          <Kbd combo="Mod+Enter" />
          {selected.altLabel ?? "Alternate"}
        </span>
      )}
      <span className="ml-auto flex items-center gap-1">
        <Kbd combo="Escape" />
        Close
      </span>
    </div>
  );
}
