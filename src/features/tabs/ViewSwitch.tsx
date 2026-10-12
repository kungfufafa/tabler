import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useTabsStore, type TableView } from "@/stores/tabs";

const VIEWS: Array<{ kind: TableView; label: string; action: "view.data" | "view.structure" }> = [
  { kind: "table", label: "Data", action: "view.data" },
  { kind: "structure", label: "Structure", action: "view.structure" },
];

/**
 * TablePlus's "Data | Structure" switch at the bottom-left of a table tab: a macOS segmented
 * control (one bordered group, the selected segment filled with the accent). Both views live in
 * the same tab; switching keeps each view's staged work.
 */
export function ViewSwitch({ tabId, view }: { tabId: string; view: TableView }) {
  const setView = useTabsStore((s) => s.setView);
  return (
    <div
      role="radiogroup"
      aria-label="Table view"
      data-slot="view-switch"
      className="border-border bg-surface divide-border flex h-5.5 shrink-0 items-stretch divide-x overflow-hidden rounded-md border shadow-xs"
    >
      {VIEWS.map((v) => {
        const keys = shortcutFor(v.action);
        const checked = v.kind === view;
        return (
          <button
            key={v.kind}
            type="button"
            role="radio"
            aria-checked={checked}
            data-state={checked ? "on" : "off"}
            title={keys ? `${v.label} (${formatCombo(keys)})` : v.label}
            onClick={() => setView(tabId, v.kind)}
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
