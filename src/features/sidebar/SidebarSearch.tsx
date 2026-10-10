import { Search, X } from "lucide-react";
import { forwardRef } from "react";
import { Kbd } from "@/components/Kbd";
import { Input } from "@/components/ui/input";

export const SidebarSearch = forwardRef<
  HTMLInputElement,
  { value: string; onChange: (v: string) => void }
>(function SidebarSearch({ value, onChange }, ref) {
  return (
    <div className="relative px-2 pb-1">
      <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-4 size-3.5 -translate-y-1/2" />
      <Input
        ref={ref}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            onChange("");
            (e.target as HTMLInputElement).blur();
          }
        }}
        placeholder="Search tables"
        className="bg-surface h-7 pr-12 pl-7 text-xs"
        data-scope="sidebar"
      />
      <div className="pointer-events-none absolute top-1/2 right-4 -translate-y-1/2">
        {value ? (
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground pointer-events-auto"
            onClick={() => onChange("")}
            aria-label="Clear"
          >
            <X className="size-3.5" />
          </button>
        ) : (
          <Kbd combo="Mod+L" />
        )}
      </div>
    </div>
  );
});
