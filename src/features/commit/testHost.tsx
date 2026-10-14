import { useQueryClient } from "@tanstack/react-query";
import { usePendingOpActions } from "@/features/objects/pendingActions";
import { CommitHost } from "./CommitHost";

/**
 * What `ObjectsHost` mounts for the commit flow — the host's ⌘S / ⇧⌘P / ⇧⌘⌫ handlers and its
 * sheets — for tests of a tab or grid that render without the whole host. Needs a
 * `QueryClientProvider` above it.
 */
export function CommitTestHost() {
  usePendingOpActions(useQueryClient());
  return <CommitHost />;
}
