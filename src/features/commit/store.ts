import { create } from "zustand";
import type { DefinitionRestore } from "@/lib/ipc";

/** Where the last workspace commit of a connection stopped (shown in its Preview sheet). */
export interface CommitFailure {
  connectionId: string;
  /** The failing part (`WorkspaceSource.key`); null when the server did not say. */
  key: string | null;
  message: string;
  /** The server's detail and hint. */
  detail: string | null;
  /** Index of the failing statement in that part's `sql`. */
  statementIndex: number | null;
  /** That part's statements as they were sent. */
  sql: string[];
  /** It all ran in one transaction, so nothing stayed ("All changes were reverted"). */
  atomic: boolean;
  /**
   * For a failed definition on MySQL (which drops the old routine before it creates the new one):
   * what became of the previous version, put back or handed back as text.
   */
  restore: DefinitionRestore | null;
}

interface CommitState {
  /** The open Preview sheet; `onCommitted` runs once after its commit went through. */
  sheet: { connectionId: string; onCommitted: (() => void) | null } | null;
  /** Connections whose workspace is being committed right now. */
  committing: Record<string, true>;
  failure: CommitFailure | null;
  /**
   * Bumped per tab once its staged work was committed or discarded: its grid clears the
   * selection of the edited rows (TablePlus #2979), its structure view resets.
   */
  generation: Record<string, number>;
}

export const useCommitStore = create<CommitState>()(() => ({
  sheet: null,
  committing: {},
  failure: null,
  generation: {},
}));

export function openWorkspaceSheet(
  connectionId: string,
  onCommitted: (() => void) | null = null,
): void {
  useCommitStore.setState({ sheet: { connectionId, onCommitted } });
}

export function closeWorkspaceSheet(): void {
  useCommitStore.setState({ sheet: null });
}

export function isCommitting(connectionId: string | null): boolean {
  return connectionId !== null && useCommitStore.getState().committing[connectionId] === true;
}

export function useCommitting(connectionId: string | null): boolean {
  return useCommitStore((s) => connectionId !== null && s.committing[connectionId] === true);
}

export function setCommitting(connectionId: string, on: boolean): void {
  useCommitStore.setState((s) => {
    if ((s.committing[connectionId] === true) === on) return s;
    const committing = { ...s.committing };
    if (on) committing[connectionId] = true;
    else delete committing[connectionId];
    return { committing };
  });
}

export function setFailure(failure: CommitFailure | null): void {
  useCommitStore.setState({ failure });
}

/** Forgets the connection's last failure. */
export function clearFailure(connectionId: string): void {
  if (useCommitStore.getState().failure?.connectionId === connectionId)
    useCommitStore.setState({ failure: null });
}

export function bumpGeneration(tabId: string): void {
  useCommitStore.setState((s) => ({
    generation: { ...s.generation, [tabId]: (s.generation[tabId] ?? 0) + 1 },
  }));
}

/** `generation` of a tab: 0 until its staged work was first committed or discarded. */
export function useCommitGeneration(tabId: string): number {
  return useCommitStore((s) => s.generation[tabId] ?? 0);
}

/** Test helper. */
export function resetCommitStore(): void {
  useCommitStore.setState({ sheet: null, committing: {}, failure: null, generation: {} });
}
