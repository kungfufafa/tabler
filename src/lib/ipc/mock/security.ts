import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { SessionDetails } from "../bindings/SessionDetails";
import { mockLaneSession, mockStoredPassword } from "./handlers";
import { mockEmit, mockError, mockInvoke, registerMock } from "./transport";

/**
 * The app lock and connection security, like `commands/lock.rs`. The passcode is kept as a salted
 * PBKDF2 hash (WebCrypto), in localStorage so `pnpm dev:mock` shows the lock screen after a
 * reload; Touch ID always succeeds after a short delay.
 */

const PASSCODE_KEY = "tabler.mock.passcode";
/** Fewer rounds than the backend: the mock only has to look the part. */
const ROUNDS = 10_000;

interface StoredPasscode {
  salt: string;
  hash: string;
}

const toHex = (bytes: ArrayBuffer | Uint8Array) =>
  Array.from(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");

async function derive(passcode: string, salt: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(passcode),
    "PBKDF2",
    false,
    ["deriveBits"],
  );
  const bits = await crypto.subtle.deriveBits(
    { name: "PBKDF2", hash: "SHA-256", salt: new TextEncoder().encode(salt), iterations: ROUNDS },
    key,
    256,
  );
  return toHex(bits);
}

let memory: StoredPasscode | null = null;

function load(): StoredPasscode | null {
  try {
    const raw = globalThis.localStorage?.getItem(PASSCODE_KEY);
    return raw ? (JSON.parse(raw) as StoredPasscode) : memory;
  } catch {
    return memory;
  }
}

function save(stored: StoredPasscode | null): void {
  memory = stored;
  try {
    if (stored) globalThis.localStorage?.setItem(PASSCODE_KEY, JSON.stringify(stored));
    else globalThis.localStorage?.removeItem(PASSCODE_KEY);
  } catch {
    // In-memory only.
  }
}

async function matches(stored: StoredPasscode, passcode: string): Promise<boolean> {
  return (await derive(passcode, stored.salt)) === stored.hash;
}

/** Test helper: no passcode. */
export function resetMockPasscode(): void {
  save(null);
}

registerMock("passcode_status", () => ({ enabled: load() !== null, biometrics: true }));

registerMock("set_passcode", async ({ current, passcode }) => {
  const stored = load();
  if (stored && !(typeof current === "string" && (await matches(stored, current))))
    mockError("AUTH_FAILED", "authentication failed: wrong passcode");
  if (passcode === null || passcode === undefined) {
    save(null);
    return null;
  }
  const next = String(passcode);
  if ([...next].length < 4) mockError("INVALID_INPUT", "The passcode needs at least 4 characters.");
  const salt = toHex(crypto.getRandomValues(new Uint8Array(16)));
  save({ salt, hash: await derive(next, salt) });
  return null;
});

registerMock("verify_passcode", async ({ passcode }) => {
  const stored = load();
  return stored !== null && (await matches(stored, String(passcode)));
});

registerMock("authenticate_biometric", async () => {
  await new Promise((r) => setTimeout(r, import.meta.env.MODE === "test" ? 0 : 600));
  return true;
});

registerMock("lock_app", () => {
  queueMicrotask(() => mockEmit("lock:system", "user"));
  return null;
});

registerMock("reveal_connection_password", async ({ profileId }) => {
  await mockInvoke<ConnectionProfile>("get_profile", { id: profileId });
  return mockStoredPassword(String(profileId));
});

registerMock("session_details", async ({ connectionId }): Promise<SessionDetails> => {
  const open = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const info = open.find((c) => c.connectionId === connectionId);
  if (!info) mockError("NOT_CONNECTED", "not connected");
  // A mock driver beyond the core ones gives its own (or has none).
  const lane = mockLaneSession(info.connectionId);
  if (lane)
    return lane.sessionDetails?.() ?? mockError("UNSUPPORTED", "unsupported: session details");
  const profile = await mockInvoke<ConnectionProfile>("get_profile", { id: info.profileId });
  if (profile.driver === "sqlite")
    return { tls: false, tlsVersion: null, tlsCipher: null, encoding: "UTF-8" };
  // "prefer" negotiates TLS with servers that offer it: the local ones in the seed do not.
  const local = ["localhost", "127.0.0.1", "::1", ""].includes(profile.host ?? "");
  const tls = profile.ssl !== "disable" && !(profile.ssl === "prefer" && local);
  return {
    tls,
    tlsVersion: tls ? "TLSv1.3" : null,
    tlsCipher: tls ? "TLS_AES_256_GCM_SHA384" : null,
    encoding: profile.driver === "postgres" ? "UTF8" : "utf8mb4",
  };
});
