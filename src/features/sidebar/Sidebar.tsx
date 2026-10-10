import { Plug } from "lucide-react";
import { useRef, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Button } from "@/components/ui/button";
import { useAction } from "@/features/palette/actions";
import { useActiveConnection } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { ConnectionSwitcher } from "./ConnectionSwitcher";
import { DatabasePicker } from "./DatabasePicker";
import { SchemaTree } from "./SchemaTree";
import { SidebarSearch } from "./SidebarSearch";
import { useCurrentSchema, useTables, useTreeRows } from "./useSchemaTree";

export function Sidebar() {
  const active = useActiveConnection();
  const setView = useUiStore((s) => s.setView);
  const [search, setSearch] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const connectionId = active?.info.connectionId ?? null;
  const schema = useCurrentSchema(connectionId);
  const tables = useTables(connectionId, schema);
  const rows = useTreeRows(tables.data, search);

  useAction("sidebar.focus-search", () => {
    searchRef.current?.focus();
    searchRef.current?.select();
  });

  return (
    <aside className="bg-sidebar text-sidebar-foreground flex h-full flex-col" data-scope="sidebar">
      {active ? (
        <>
          <ConnectionSwitcher />
          <DatabasePicker connection={active} />
          <SidebarSearch ref={searchRef} value={search} onChange={setSearch} />
          <SchemaTree
            connectionId={active.info.connectionId}
            rows={rows}
            loading={tables.isPending && schema !== null}
            error={tables.isError ? tables.error.message : null}
            search={search}
          />
        </>
      ) : (
        <EmptyState
          icon={Plug}
          title="No connection"
          description="Open a connection to browse its tables."
        >
          <Button size="sm" variant="outline" onClick={() => setView("launcher")}>
            Connections
          </Button>
        </EmptyState>
      )}
    </aside>
  );
}
