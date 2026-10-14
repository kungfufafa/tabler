import { SSH_PROMPT_HOST } from "@/features/connections/sshPromptStore";
import { hasSingleHost } from "@/features/export/singleHost";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { SshPromptRequest } from "../bindings/SshPromptRequest";
import { mockEmit, mockError, registerMock } from "./transport";

/**
 * Mock parts of connect / test_connection beyond the basics (see handlers.ts):
 * - "Command Line (IAM authentication)": any command "prints" a fixed fake token; a command
 *   containing `fail` fails like a CLI without credentials.
 * - MySQL "Use socket": over SSH the path is the SSH server's (full path or none); a socket path
 *   containing `missing` does not exist.
 * - SSH keyboard-interactive: "Staging via SSH" asks for a "Verification code" (any 6 digits
 *   pass) when the prompt host is mounted (the app; tests without it are not asked).
 * - Bootstrap CLI script: a script containing `fail` fails like rsync without a server.
 * - SQLCipher: SQLite files created with "New…" and a key, and paths containing `encrypted`
 *   (key `secret`), need their key (the profile's password).
 * - PostgreSQL SSL negotiation DIRECT is refused with SSL mode disable / prefer.
 */

/** What every mock password command prints. */
export const MOCK_IAM_TOKEN = "mock-rds-iam-token";

/** Runs the profile's password command (Command Line mode): the token, or null in other modes. */
export function mockPasswordCommand(p: ConnectionProfile): string | null {
  if (p.driver === "sqlite" || p.options.passwordMode !== "command") return null;
  const command = p.options.passwordCommand?.trim() ?? "";
  if (command === "")
    mockError("INVALID_INPUT", "Enter the command that prints the password (Command Line mode)");
  if (command.includes("fail"))
    mockError(
      "CONNECT_FAILED",
      'the password command failed (exit status 255): Unable to locate credentials. You can configure credentials by running "aws configure".',
    );
  return MOCK_IAM_TOKEN;
}

/** Mirrors the backend's checks of a MySQL "Use socket" profile. */
export function mockSocketCheck(p: ConnectionProfile): void {
  if (p.driver !== "mysql" || p.options.useSocket !== true) return;
  const typed = p.options.socketPath?.trim() ?? "";
  if (p.ssh && typed !== "" && !typed.startsWith("/"))
    mockError(
      "INVALID_INPUT",
      `Over SSH the socket path is the one on the SSH server: enter its full path (e.g. /var/run/mysqld/mysqld.sock), not “${typed}”`,
    );
  const path = typed || (p.ssh ? "/var/run/mysqld/mysqld.sock" : "/tmp/mysql.sock");
  if (path.includes("missing"))
    mockError("CONNECT_FAILED", `MySQL socket not found: ${path} (is the server running?)`);
}

/** Profiles whose SSH server asks for a one-time code. */
const TWO_FACTOR_PROFILES = new Set(["p-staging-ssh"]);
const waiting = new Map<string, (answers: string[] | null) => void>();

/**
 * The SSH server's keyboard-interactive question of a 2FA profile: resolves once a 6-digit code
 * is answered; throws CANCELLED (cancelled) or AUTH_FAILED (anything else).
 */
export async function mockSshPrompt(
  p: ConnectionProfile,
  purpose: "connect" | "test",
): Promise<void> {
  const ssh = p.driver === "sqlite" ? null : p.ssh;
  if (!ssh || !TWO_FACTOR_PROFILES.has(p.id) || !hasSingleHost(SSH_PROMPT_HOST)) return;
  const request: SshPromptRequest = {
    requestId: crypto.randomUUID(),
    profileId: p.id,
    label: p.name,
    purpose,
    user: ssh.user || "deploy",
    host: ssh.host,
    port: ssh.port || 22,
    name: "",
    instructions: "Two-factor authentication is required.\nOpen your authenticator app.",
    prompts: [{ prompt: "Verification code: ", echo: false }],
  };
  const answers = await new Promise<string[] | null>((resolve) => {
    waiting.set(request.requestId, resolve);
    mockEmit("ssh:prompt", request);
  });
  mockEmit("ssh:prompt-done", { requestId: request.requestId });
  if (answers === null) mockError("CANCELLED", "query cancelled");
  if (!/^\d{6}$/.test(answers[0]?.trim() ?? ""))
    mockError(
      "AUTH_FAILED",
      `authentication failed: ssh: keyboard-interactive authentication failed for user "${request.user}"; the server accepts: keyboard-interactive`,
    );
}

registerMock("answer_ssh_prompt", ({ requestId, answers }) => {
  const resolve = waiting.get(requestId as string);
  waiting.delete(requestId as string);
  resolve?.(Array.isArray(answers) ? (answers as string[]) : null);
});

/** Mirrors the backend's bootstrap CLI script run before a connect or test. */
export function mockBootstrapScript(p: ConnectionProfile): void {
  const script = p.options.bootstrapScript?.trim() ?? "";
  if (script.includes("fail"))
    mockError(
      "CONNECT_FAILED",
      "the bootstrap script failed (exit status 12): rsync: connection refused (code 10)",
    );
}

/** SQLCipher databases of the mock: path → key (files created with a key in this session). */
const encryptedFiles = new Map<string, string>();

/** "New…" with a key creates an encrypted database. */
export function mockCreateSqliteFile(path: string, key: string | null): void {
  if (key) encryptedFiles.set(path, key);
  else encryptedFiles.delete(path);
}

/** Mirrors the backend's SQLCipher checks of a SQLite connect (`key`: the profile's password). */
export function mockSqliteKey(p: ConnectionProfile, key: string | null | undefined): void {
  if (p.driver !== "sqlite" || p.ssh) return;
  const path = p.sqlitePath ?? "";
  const expected = encryptedFiles.get(path) ?? (path.includes("encrypted") ? "secret" : null);
  if (expected === null) {
    if (key)
      mockError(
        "CONNECT_FAILED",
        "This database is not encrypted: remove the encryption key from the connection",
      );
    return;
  }
  if (!key)
    mockError(
      "CONNECT_FAILED",
      "This database is encrypted (SQLCipher): enter its encryption key in the connection's settings",
    );
  if (key !== expected)
    mockError("CONNECT_FAILED", "Wrong encryption key (or not a SQLCipher database)");
}

/** Mirrors the backend's PostgreSQL option checks (`check_postgres`). */
export function mockDriverOptions(p: ConnectionProfile): void {
  if (
    p.driver === "postgres" &&
    p.options.pgSslNegotiation === "direct" &&
    (p.ssl === "disable" || p.ssl === "prefer")
  )
    mockError(
      "INVALID_INPUT",
      "SSL negotiation DIRECT needs the SSL mode require, verify-ca or verify-full",
    );
}
