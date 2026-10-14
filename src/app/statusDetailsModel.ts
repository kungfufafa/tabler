import { useQuery } from "@tanstack/react-query";
import { ipc, type ConnectionProfile, type SessionDetails } from "@/lib/ipc";
import type { AccentPreset } from "@/lib/theme";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";

export const sessionDetailsKey = (connectionId: string) =>
  ["session-details", connectionId] as const;

/**
 * TLS state and character set of a session, read once per session (a reconnect gets a new id).
 * Only while it is connected; null until known.
 */
export function useSessionDetails(conn: OpenConnection | null): SessionDetails | null {
  const id = conn?.info.connectionId ?? null;
  const query = useQuery({
    queryKey: sessionDetailsKey(id ?? ""),
    queryFn: () => ipc.security.sessionDetails(id!),
    enabled: id !== null && conn?.status === "connected",
    staleTime: Infinity,
    retry: false,
  });
  return query.data ?? null;
}

/** Where the connection points: socket / file / host:port. */
export function hostLabel(profile: ConnectionProfile | null): string | null {
  if (!profile) return null;
  if (profile.driver === "sqlite") return profile.sqlitePath;
  const socket = profile.options.socketPath;
  if (profile.options.useSocket && socket) return socket;
  if (!profile.host) return null;
  return profile.port ? `${profile.host}:${profile.port}` : profile.host;
}

/** "TLSv1.3 · TLS_AES_256_GCM_SHA384", "Not encrypted", or "…" while unknown. */
export function tlsLabel(details: SessionDetails | null, driver: string): string {
  if (driver === "sqlite") return "Local file";
  if (!details) return "…";
  if (!details.tls) return "Not encrypted";
  return [details.tlsVersion ?? "TLS", details.tlsCipher].filter(Boolean).join(" · ");
}

/** "user@host:22 → localhost:50000" or "None". */
export function tunnelLabel(conn: OpenConnection, profile: ConnectionProfile | null): string {
  const port = conn.info.tunnel?.localPort;
  if (port === undefined) return "None";
  const ssh = profile?.ssh;
  const server = ssh ? `${ssh.user ? `${ssh.user}@` : ""}${ssh.host}:${ssh.port}` : "SSH";
  return `${server} → localhost:${port}`;
}

/**
 * Saves `color` on the connection's profile and shows it right away on every open connection of
 * that profile (the pill, the rail, the accent).
 */
export async function recolorConnection(
  profileId: string,
  color: AccentPreset | string,
): Promise<void> {
  const profile = await ipc.profiles.get(profileId);
  if (profile.color === color) return;
  await ipc.profiles.save({
    profile: { ...profile, color },
    password: null,
    sshPassword: null,
    sshPassphrase: null,
    clearPassword: false,
  });
  const store = useConnectionsStore.getState();
  for (const c of Object.values(store.byId))
    if (c.info.profileId === profileId) store.setInfo({ ...c.info, color });
}
