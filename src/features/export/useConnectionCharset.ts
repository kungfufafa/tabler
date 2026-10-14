import { useQuery } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc";

/**
 * The charset the connection's session uses now (Connection ▸ View Using Encoding…: `utf8mb4`,
 * `latin1`, `UTF8`…), or null while it loads, for SQLite (its text is always UTF-8) and when the
 * session cannot say. The query key holds the connection id: a change of encoding reloads it with
 * the rest of the connection's data.
 */
export function useConnectionCharset(connectionId: string): string | null {
  const q = useQuery({
    queryKey: ["export", "connectionEncoding", connectionId] as const,
    queryFn: () => ipc.connections.getEncoding(connectionId),
    retry: false,
    staleTime: 0,
  });
  return q.data ?? null;
}
