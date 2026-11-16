import { RotateCw, Search } from "lucide-react";
import { useMemo, useState, type KeyboardEvent } from "react";
import { FormError } from "@/components/FormError";
import { QuietStatus } from "@/components/QuietLoading";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { ColumnHeader } from "./parts";

/** Column 2: the selected connection's databases, with search. */
export function DatabaseList({
  databases,
  selected,
  current,
  loading,
  error,
  onSelect,
  onRetry,
  disabled,
}: {
  databases: readonly string[];
  selected: string | null;
  /** The connection's default database (marked). */
  current: string | null;
  loading: boolean;
  error: string | null;
  onSelect: (name: string) => void;
  onRetry: () => void;
  disabled?: boolean;
}) {
  const [query, setQuery] = useState("");
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return q ? databases.filter((d) => d.toLowerCase().includes(q)) : databases;
  }, [databases, query]);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const i = selected === null ? -1 : shown.indexOf(selected);
    const next =
      shown[e.key === "ArrowDown" ? Math.min(shown.length - 1, i + 1) : Math.max(0, i - 1)];
    if (next !== undefined) onSelect(next);
  };

  return (
    <div className="border-border flex min-h-0 flex-col border-r">
      <ColumnHeader>Databases</ColumnHeader>
      <div className="px-2 pt-2 pb-1">
        <div className="relative">
          <Search
            aria-hidden
            className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2"
          />
          <Input
            aria-label="Search databases"
            placeholder="Search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            disabled={disabled || loading || error !== null}
            spellCheck={false}
            autoComplete="off"
            className="h-7 rounded-md pl-7 text-xs md:text-xs"
          />
        </div>
      </div>
      <div
        role="listbox"
        aria-label="Databases"
        tabIndex={0}
        aria-activedescendant={selected ? `backup-db-${selected}` : undefined}
        onKeyDown={onKeyDown}
        className={cn(
          "focus-visible:ring-ring/50 min-h-0 flex-1 overflow-y-auto pb-1 outline-none focus-visible:ring-2 focus-visible:ring-inset",
          disabled && "pointer-events-none opacity-50",
        )}
      >
        {loading && (
          <QuietStatus delay={400} className="block px-3 py-1.5">
            Loading databases
          </QuietStatus>
        )}
        {error !== null && (
          <div className="flex flex-col items-start gap-2 px-3 py-2">
            <FormError title={error} className="[&>span]:line-clamp-4">
              {error}
            </FormError>
            <Button type="button" variant="outline" size="xs" onClick={onRetry}>
              <RotateCw /> Try Again
            </Button>
          </div>
        )}
        {!loading && error === null && shown.length === 0 && (
          <p className="text-muted-foreground px-3 py-2 text-xs">
            {query ? "No matches" : "No databases"}
          </p>
        )}
        {shown.map((name) => (
          <div
            key={name}
            id={`backup-db-${name}`}
            role="option"
            aria-selected={name === selected}
            onClick={() => onSelect(name)}
            className={cn(
              "mx-1 flex h-7 cursor-default items-center gap-2 rounded-md px-2 font-mono text-xs transition-colors",
              name === selected ? "bg-brand-soft text-foreground" : "hover:bg-row-hover",
            )}
          >
            <span className="min-w-0 flex-1 truncate" title={name}>
              {name}
            </span>
            {name === current && (
              <span className="border-border text-muted-foreground inline-flex h-4 shrink-0 items-center rounded-sm border px-1 font-sans text-[10px] leading-none font-medium">
                default
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
