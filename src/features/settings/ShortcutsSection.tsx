import { Search } from "lucide-react";
import { useMemo, type Ref } from "react";
import { Kbd } from "@/components/Kbd";
import { Input } from "@/components/ui/input";
import { isMac } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { SectionShell, SettingGroup } from "./controls";
import { groupShortcuts } from "./shortcuts";

const LEGEND = isMac ? "⌘ Command  ·  ⌥ Option  ·  ⇧ Shift  ·  ⌃ Control" : null;

export function ShortcutsSection({
  query,
  onQueryChange,
  searchRef,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  searchRef?: Ref<HTMLInputElement>;
}) {
  const groups = useMemo(() => groupShortcuts(KEYMAP, query), [query]);
  return (
    <SectionShell
      title="Shortcuts"
      actions={
        <div className="relative">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
          <Input
            ref={searchRef}
            type="search"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder="Search shortcuts"
            aria-label="Search shortcuts"
            spellCheck={false}
            autoComplete="off"
            className="h-7 w-52 rounded-md pl-7 text-xs md:text-xs [&::-webkit-search-cancel-button]:hidden"
          />
        </div>
      }
    >
      {groups.length === 0 ? (
        <p className="text-muted-foreground py-10 text-center text-xs">
          No shortcuts match “{query.trim()}”.
        </p>
      ) : (
        groups.map((g) => (
          <SettingGroup key={g.scope} title={g.title}>
            {g.rows.map((row) => (
              <div
                key={`${row.id}:${row.label}`}
                className="flex h-7 items-center gap-3 px-3 text-[13px]"
              >
                <span className="min-w-0 flex-1 truncate">{row.label}</span>
                <span className="flex shrink-0 items-center gap-1">
                  {row.keys.map((k) => (
                    <Kbd
                      key={k}
                      combo={k}
                      className="bg-surface-2 text-foreground/80 h-5 min-w-5 px-1.5 text-[11px]"
                    />
                  ))}
                </span>
              </div>
            ))}
          </SettingGroup>
        ))
      )}
      {LEGEND && (
        <p className="text-muted-foreground mt-auto px-1 text-[11px] whitespace-pre">{LEGEND}</p>
      )}
    </SectionShell>
  );
}
