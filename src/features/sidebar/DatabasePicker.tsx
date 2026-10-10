import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useCurrentSchema, useDatabases, useSchemas } from "./useSchemaTree";

/** Database selector (PG/MySQL) plus schema selector (PG). SQLite shows the attached databases. */
export function DatabasePicker({ connection }: { connection: OpenConnection }) {
  const id = connection.info.connectionId;
  const qc = useQueryClient();
  const databases = useDatabases(id);
  const schemas = useSchemas(id);
  const currentSchema = useCurrentSchema(id);
  const setCurrentSchema = useConnectionsStore((s) => s.setCurrentSchema);
  const setInfo = useConnectionsStore((s) => s.setInfo);
  const closeForConnection = useTabsStore((s) => s.closeForConnection);
  const switchDb = useMutation({
    mutationFn: (database: string) => ipc.connections.switchDatabase(id, database),
    onSuccess: (info) => {
      setInfo(info);
      setCurrentSchema(id, null);
      closeForConnection(id);
      void qc.invalidateQueries({ queryKey: ["schemas", id] });
      void qc.invalidateQueries({ queryKey: ["tables", id] });
      void qc.invalidateQueries({ queryKey: ["databases", id] });
    },
    onError: (e) => toast.error("Could not switch database", { description: e.message }),
  });
  const driver = connection.info.driver;
  const showDb = driver !== "sqlite";
  const showSchema = driver === "postgres" || driver === "sqlite";
  return (
    <div className="flex flex-col gap-1.5 px-2 pb-2">
      {showDb && (
        <Select
          value={connection.info.currentDatabase ?? ""}
          onValueChange={(v) => switchDb.mutate(v)}
          disabled={switchDb.isPending}
        >
          <SelectTrigger size="sm" className="bg-surface h-7 w-full text-xs">
            <SelectValue placeholder="Database" />
          </SelectTrigger>
          <SelectContent>
            {(databases.data ?? []).map((d) => (
              <SelectItem key={d.name} value={d.name} className="text-xs">
                {d.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {showSchema && (schemas.data?.length ?? 0) > 1 && (
        <Select value={currentSchema ?? ""} onValueChange={(v) => setCurrentSchema(id, v)}>
          <SelectTrigger size="sm" className="bg-surface h-7 w-full text-xs">
            <SelectValue placeholder="Schema" />
          </SelectTrigger>
          <SelectContent>
            {(schemas.data ?? []).map((s) => (
              <SelectItem key={s.name} value={s.name} className="text-xs">
                {s.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}
