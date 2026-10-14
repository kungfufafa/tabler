import { useSyncExternalStore } from "react";
import { actions, type ActionId } from "@/features/palette/actions";

/** Whether something handles `id` right now (re-renders when handlers come and go). */
export function useActionAvailable(id: ActionId): boolean {
  return useSyncExternalStore(
    (onChange) => actions.subscribe(onChange),
    () => actions.has(id),
  );
}
