import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { toast } from "sonner";
import { ipc, type ProfileInput } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { closeConnectionTabs } from "./closeGuard";

export const profileKeys = {
  all: ["profiles"] as const,
  secret: (id: string) => ["profiles", id, "secret"] as const,
};

export function useProfiles() {
  return useQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 });
}

export function useSecretStatus(id: string | null) {
  return useQuery({
    queryKey: profileKeys.secret(id ?? ""),
    queryFn: () => ipc.profiles.secretStatus(id!),
    enabled: id !== null,
  });
}

export function useSaveProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: ProfileInput) => ipc.profiles.save(input),
    onSuccess: (profile) => {
      void qc.invalidateQueries({ queryKey: profileKeys.all });
      void qc.invalidateQueries({ queryKey: profileKeys.secret(profile.id) });
    },
    onError: (e) => toast.error("Could not save connection", { description: e.message }),
  });
}

export function useDeleteProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ipc.profiles.delete(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: profileKeys.all }),
    onError: (e) => toast.error("Could not delete connection", { description: e.message }),
  });
}

export function useDuplicateProfile() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => ipc.profiles.duplicate(id),
    onSuccess: () => void qc.invalidateQueries({ queryKey: profileKeys.all }),
    onError: (e) => toast.error("Could not duplicate connection", { description: e.message }),
  });
}

export function useTestConnection() {
  return useMutation({ mutationFn: (input: ProfileInput) => ipc.profiles.test(input) });
}

/** Opens a connection, registers it in the store and switches to the workspace. */
export function useConnect() {
  const qc = useQueryClient();
  const add = useConnectionsStore((s) => s.add);
  const setView = useUiStore((s) => s.setView);
  return useMutation({
    mutationFn: (profileId: string) => ipc.connections.connect(profileId),
    onSuccess: (info) => {
      add(info);
      setView("workspace");
      void qc.invalidateQueries({ queryKey: profileKeys.all });
    },
    onError: (e, profileId) => {
      const name = qc
        .getQueryData<{ id: string; name: string }[]>(profileKeys.all)
        ?.find((p) => p.id === profileId)?.name;
      toast.error(`Could not connect${name ? ` to ${name}` : ""}`, { description: e.message });
    },
  });
}

/**
 * Raw disconnect: drops the connection's tabs without asking. UI entry points go through
 * `useGuardedDisconnect` so unsaved work gets the "Discard changes?" prompt first.
 */
export function useDisconnect() {
  const remove = useConnectionsStore((s) => s.remove);
  const closeForConnection = useTabsStore((s) => s.closeForConnection);
  const setView = useUiStore((s) => s.setView);
  return useMutation({
    mutationFn: (connectionId: string) => ipc.connections.disconnect(connectionId),
    onSettled: (_data, _err, connectionId) => {
      closeForConnection(connectionId);
      remove(connectionId);
      if (useConnectionsStore.getState().order.length === 0) setView("launcher");
    },
  });
}

/**
 * Disconnect behind the unsaved-changes guard: the connection's tabs are closed first (asking when
 * any hold staged edits or an unsaved query). Resolves false when the user kept them.
 */
export function useGuardedDisconnect(): (connectionId: string) => Promise<boolean> {
  const { mutate } = useDisconnect();
  return useCallback(
    (connectionId: string) => closeConnectionTabs(connectionId, () => mutate(connectionId)),
    [mutate],
  );
}
