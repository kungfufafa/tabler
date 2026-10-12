import { useQuery } from "@tanstack/react-query";
import { withHostKeyTrust } from "@/features/connections/useConnections";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import type { DumpDriver } from "./options";

export const backupKeys = {
  binaries: (driver: DumpDriver) => ["backup", "binaries", driver] as const,
  target: (profileId: string) => ["backup", "target", profileId] as const,
};

/** The driver's dump / restore binaries on this Mac (rescanned when the window reopens). */
export function useDumpBinaries(driver: DumpDriver | null) {
  return useQuery({
    queryKey: backupKeys.binaries(driver ?? "postgres"),
    queryFn: () => ipc.backup.binaries(driver!),
    enabled: driver !== null,
    staleTime: 60_000,
    retry: false,
  });
}

/**
 * The server version and databases of a saved PostgreSQL / MySQL connection: through its open
 * connection, or a temporary one (SSH host keys are confirmed like on connect).
 */
export function useBackupTarget(profile: ConnectionProfile | null) {
  const enabled = profile !== null && profile.driver !== "sqlite";
  return useQuery({
    queryKey: backupKeys.target(profile?.id ?? ""),
    queryFn: () =>
      withHostKeyTrust(profile!.id, "connect", profile!.name, () =>
        ipc.backup.inspectTarget(profile!.id),
      ),
    enabled,
    staleTime: 30_000,
    retry: false,
  });
}
