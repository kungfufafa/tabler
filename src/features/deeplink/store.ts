import { create } from "zustand";

/** A connection opened from a URL and not saved: its URL passwords stay in memory for "Save". */
export interface TemporaryConnection {
  profileId: string;
  name: string;
  password: string | null;
  sshPassword: string | null;
  /** Once connected: when its last connection closes, the profile is removed. */
  seenConnected: boolean;
}

/** An `.sql` (`.sql.gz`) script waiting for the user to pick a connection. */
export interface PendingFile {
  path: string;
  /** When it was set: the wait for a connection ends after a while. */
  since: number;
}

/**
 * A link's raw SQL condition waiting for the user's yes (`FilterConfirmDialog`): the table opened
 * with it unchecked in the filter bar; "Apply filter" checks it.
 */
export interface FilterRequest {
  id: number;
  /** The connection's and the table's names, as the dialog shows them. */
  connection: string;
  table: string;
  /** The link's SQL condition, verbatim. */
  sql: string;
}

interface DeepLinkState {
  temporary: Record<string, TemporaryConnection>;
  pendingFile: PendingFile | null;
  /** Raw conditions to confirm, oldest first (the dialog shows the first). */
  filterRequests: FilterRequest[];
}

export const useDeepLinkStore = create<DeepLinkState>()(() => ({
  temporary: {},
  pendingFile: null,
  filterRequests: [],
}));

let nextFilterRequest = 0;
const filterResolvers = new Map<number, (apply: boolean) => void>();

/**
 * Asks whether to apply a link's raw SQL condition; resolves with the answer (false: "Not now",
 * or the request was dropped).
 */
export function askApplyFilter(request: Omit<FilterRequest, "id">): Promise<boolean> {
  const id = ++nextFilterRequest;
  return new Promise((resolve) => {
    filterResolvers.set(id, resolve);
    useDeepLinkStore.setState((s) => ({
      filterRequests: [...s.filterRequests, { ...request, id }],
    }));
  });
}

/** Answers request `id`; a later answer for it is ignored. */
export function answerFilterRequest(id: number, apply: boolean): void {
  const resolve = filterResolvers.get(id);
  filterResolvers.delete(id);
  useDeepLinkStore.setState((s) =>
    s.filterRequests.some((r) => r.id === id)
      ? { filterRequests: s.filterRequests.filter((r) => r.id !== id) }
      : s,
  );
  resolve?.(apply);
}

export function rememberTemporary(t: TemporaryConnection): void {
  useDeepLinkStore.setState((s) => ({ temporary: { ...s.temporary, [t.profileId]: t } }));
}

export function forgetTemporary(profileId: string): void {
  useDeepLinkStore.setState((s) => {
    if (!(profileId in s.temporary)) return s;
    const temporary = { ...s.temporary };
    delete temporary[profileId];
    return { temporary };
  });
}

export function markSeenConnected(profileId: string): void {
  useDeepLinkStore.setState((s) => {
    const t = s.temporary[profileId];
    if (!t || t.seenConnected) return s;
    return { temporary: { ...s.temporary, [profileId]: { ...t, seenConnected: true } } };
  });
}

export function isTemporary(profileId: string): boolean {
  return profileId in useDeepLinkStore.getState().temporary;
}

export function setPendingFile(file: PendingFile | null): void {
  useDeepLinkStore.setState({ pendingFile: file });
}
