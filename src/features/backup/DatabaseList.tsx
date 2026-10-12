import { CircleAlert, Loader2, RotateCw, Search } from "lucide-react";
import { useMemo, useState, type KeyboardEvent } from "react";
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
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
          <Input
            aria-label="Search databases"
            placeholder="Search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            disabled={disabled || loading || error !== null}
            spellCheck={false}
            autoComplete="off"
            className="h-7 pl-7 text-xs"
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
          "min-h-0 flex-1 overflow-y-auto pb-1 outline-none",
          disabled && "pointer-events-none opacity-60",
        )}
      >
        {loading && (
          <div className="text-muted-foreground flex items-center gap-2 px-3 py-2 text-xs">
            <Loader2 className="size-3.5 animate-spin" /> Loading databases…
          </div>
        )}
        {error !== null && (
          <div role="alert" className="flex flex-col items-start gap-2 px-3 py-2 text-xs">
            <span className="text-error flex items-start gap-1.5">
              <CircleAlert className="mt-px size-3.5 shrink-0" />
              <span className="line-clamp-4 break-words" title={error}>
                {error}
              </span>
            </span>
            <Button type="button" variant="outline" size="xs" onClick={onRetry}>
              <RotateCw /> Try again
            </Button>
          </div>
        )}
        {!loading && error === null && shown.length === 0 && (
          <p className="text-muted-foreground px-3 py-2 text-xs">
            {query ? "No database matches." : "No databases."}
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
              "mx-1 flex h-7 cursor-default items-center gap-2 rounded-md px-2 font-mono text-xs",
              name === selected ? "bg-brand-soft text-foreground" : "hover:bg-row-hover",
            )}
          >
            <span className="min-w-0 flex-1 truncate" title={name}>
              {name}
            </span>
            {name === current && (
              <span className="text-muted-foreground shrink-0 font-sans text-[10px]">default</span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
