import { SSH_PROMPT_HOST } from "@/features/connections/sshPromptStore";
import { hasSingleHost } from "@/features/export/singleHost";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { SshPromptRequest } from "../bindings/SshPromptRequest";
import { mockEmit, mockError, registerMock } from "./transport";

/**
 * Mock parts of connect / test_connection beyond the basics (see handlers.ts):
 * - "Command Line (IAM authentication)": any command "prints" a fixed fake token; a command
 *   containing `fail` fails like a CLI without credentials.
 * - MySQL "Use socket": refused over SSH; a socket path containing `missing` does not exist.
 * - SSH keyboard-interactive: "Staging via SSH" asks for a "Verification code" (any 6 digits
 *   pass) when the prompt host is mounted (the app; tests without it are not asked).
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
  if (p.ssh)
    mockError(
      "INVALID_INPUT",
      "A MySQL socket connection cannot go through an SSH tunnel yet: turn off “Use socket” or “Over SSH”",
    );
  const path = p.options.socketPath?.trim() || "/tmp/mysql.sock";
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
