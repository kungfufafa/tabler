import { useMemo, type KeyboardEvent } from "react";
import { DriverBadge } from "@/features/connections/badges";
import { groupProfiles } from "@/features/connections/list";
import type { ConnectionProfile } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { ColumnHeader } from "./parts";

/** Column 1: the saved PostgreSQL / MySQL connections, grouped like the welcome screen. */
export function ConnectionList({
  profiles,
  selectedId,
  onSelect,
  disabled,
}: {
  profiles: readonly ConnectionProfile[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  disabled?: boolean;
}) {
  const groups = useMemo(() => groupProfiles(profiles, ""), [profiles]);
  const flat = useMemo(() => groups.flatMap(([, items]) => items), [groups]);
  const openProfiles = useConnectionsStore((s) =>
    Object.values(s.byId)
      .map((c) => c.info.profileId)
      .sort()
      .join("\n"),
  );
  const isOpen = (id: string) => openProfiles.split("\n").includes(id);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const i = flat.findIndex((p) => p.id === selectedId);
    const next =
      flat[e.key === "ArrowDown" ? Math.min(flat.length - 1, i + 1) : Math.max(0, i - 1)];
    if (next) onSelect(next.id);
  };

  return (
    <div className="border-border flex min-h-0 flex-col border-r">
      <ColumnHeader>Connections</ColumnHeader>
      <div
        role="listbox"
        aria-label="Connections"
        tabIndex={0}
        aria-activedescendant={selectedId ? `backup-conn-${selectedId}` : undefined}
        onKeyDown={onKeyDown}
        className={cn(
          "min-h-0 flex-1 overflow-y-auto py-1 outline-none",
          disabled && "pointer-events-none opacity-60",
        )}
      >
        {flat.length === 0 && (
          <p className="text-muted-foreground px-3 py-2 text-xs">
            No PostgreSQL or MySQL connections yet.
          </p>
        )}
        {groups.map(([group, items]) => (
          <div key={group || "(none)"} role="group" aria-label={group || "Connections"}>
            {group && (
              <div className="text-muted-foreground px-3 pt-2 pb-0.5 text-[10px] font-semibold tracking-wide uppercase">
                {group}
              </div>
            )}
            {items.map((p) => {
              const selected = p.id === selectedId;
              return (
                <div
                  key={p.id}
                  id={`backup-conn-${p.id}`}
                  role="option"
                  aria-selected={selected}
                  onClick={() => onSelect(p.id)}
                  className={cn(
                    "mx-1 flex h-8 cursor-default items-center gap-2 rounded-md px-1.5 text-xs",
                    selected ? "bg-brand-soft text-foreground" : "hover:bg-row-hover",
                  )}
                >
                  <DriverBadge
                    driver={p.driver}
                    mysqlVersion={p.options.mysqlVersion}
                    pgFlavor={p.options.pgFlavor}
                    color={p.color}
                    className="size-5 rounded text-[8px]"
                  />
                  <span className="min-w-0 flex-1 truncate">{p.name}</span>
                  {isOpen(p.id) && (
                    <span
                      className="bg-success size-1.5 shrink-0 rounded-full"
                      role="img"
                      aria-label="open"
                      title="Open"
                    />
                  )}
                </div>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
