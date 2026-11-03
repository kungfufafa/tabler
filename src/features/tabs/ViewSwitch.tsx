import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useTabsStore, type StructureSection, type TableView } from "@/stores/tabs";

/** The footer's three segments: a view, and for the structure view what it shows. */
export type ViewSegment = "table" | "structure" | "indexes";

const VIEWS: Array<{
  segment: ViewSegment;
  label: string;
  kind: TableView;
  section: StructureSection;
  action?: "view.data" | "view.structure";
}> = [
  { segment: "table", label: "Data", kind: "table", section: "columns", action: "view.data" },
  {
    segment: "structure",
    label: "Structure",
    kind: "structure",
    section: "columns",
    action: "view.structure",
  },
  { segment: "indexes", label: "Index", kind: "structure", section: "indexes" },
];

/**
 * TablePlus's "Data | Structure | Index" switch at the bottom-left of a table tab: a macOS
 * segmented control (one bordered group, the selected segment filled with the accent). All three
 * live in the same tab; switching keeps each view's staged work. Index is the structure view
 * showing the indexes alone.
 */
export function ViewSwitch({
  tabId,
  view,
  section = "columns",
}: {
  tabId: string;
  view: TableView;
  /** What a structure view shows (`columns` unless the Index segment is on). */
  section?: StructureSection;
}) {
  const setView = useTabsStore((s) => s.setView);
  const current: ViewSegment =
    view === "table" ? "table" : section === "indexes" ? "indexes" : "structure";
  return (
    <div
      role="radiogroup"
      aria-label="Table view"
      data-slot="view-switch"
      className="border-border bg-surface divide-border flex h-5.5 shrink-0 items-stretch divide-x overflow-hidden rounded-md border shadow-xs"
    >
      {VIEWS.map((v) => {
        const keys = v.action ? shortcutFor(v.action) : null;
        const checked = v.segment === current;
        return (
          <button
            key={v.segment}
            type="button"
            role="radio"
            aria-checked={checked}
            data-state={checked ? "on" : "off"}
            title={keys ? `${v.label} (${formatCombo(keys)})` : v.label}
            onClick={() => setView(tabId, v.kind, v.section)}
            className={cn(
              // Inset focus ring: the group clips anything drawn outside a segment.
              "min-w-14 px-2.5 text-[11px] leading-none font-medium outline-none focus-visible:ring-2 focus-visible:ring-inset",
              checked
                ? "bg-primary text-primary-foreground focus-visible:ring-primary-foreground/60"
                : "text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:ring-ring/50",
            )}
          >
            {v.label}
          </button>
        );
      })}
    </div>
  );
}
