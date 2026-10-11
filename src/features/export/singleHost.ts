import { useEffect, useState, useSyncExternalStore } from "react";

/**
 * "Only one of these renders" for app-wide hosts (the export dialog, the SSH host-key prompt) that
 * may be mounted from more than one place while they move to a single mount point: the first
 * mounted instance owns the slot, later ones render nothing until it unmounts.
 */
const slots = new Map<string, symbol[]>();
const listeners = new Set<() => void>();

const notify = () => listeners.forEach((l) => l());
const subscribe = (onChange: () => void) => {
  listeners.add(onChange);
  return () => void listeners.delete(onChange);
};

/** Whether any instance of the host `name` is mounted (e.g. so a prompt can fall back to a toast). */
export function hasSingleHost(name: string): boolean {
  return (slots.get(name)?.length ?? 0) > 0;
}

/** Registers this component as a candidate host; true while it is the one that should render. */
export function useSingleHost(name: string): boolean {
  const [token] = useState(() => Symbol(name));
  useEffect(() => {
    const list = slots.get(name) ?? [];
    list.push(token);
    slots.set(name, list);
    notify();
    return () => {
      const current = slots.get(name);
      const i = current?.indexOf(token) ?? -1;
      if (current && i >= 0) current.splice(i, 1);
      if (current?.length === 0) slots.delete(name);
      notify();
    };
  }, [name, token]);
  return useSyncExternalStore(
    subscribe,
    () => slots.get(name)?.[0] === token,
    () => false,
  );
}
