import { ipc, isMock, type DumpBinary, type DumpTool, type ParsedVersion } from "@/lib/ipc";
import { isOlderThanServer, serverLabel } from "./options";

/** One "Install with Homebrew…" choice. */
export interface BrewChoice {
  formula: string;
  label: string;
}

/** What the backup window offers when no usable dump / restore binary is installed. */
export interface InstallHelp {
  /** Why: none found, or only versions older than the server. */
  reason: string;
  /** The first is the main button; the others are alternatives. */
  formulas: BrewChoice[];
  download: { label: string; url: string };
}

const PG_TOOLS: readonly DumpTool[] = ["pg_dump", "pg_restore", "psql"];

/**
 * The install help for `tool` (TablePlus bundles the binaries; Tabler uses installed ones): null
 * while one of them can serve `server` (not older than it; any one when the server is unknown).
 */
export function installHelp(
  tool: DumpTool,
  binaries: readonly DumpBinary[],
  server: ParsedVersion | null,
): InstallHelp | null {
  const list = binaries.filter((b) => b.tool === tool);
  const usable = server
    ? list.some((b) => b.version?.flavor === server.flavor && !isOlderThanServer(b, server))
    : list.length > 0;
  if (usable) return null;
  const reason =
    list.length === 0
      ? `No ${tool} found in PATH, Homebrew, Postgres.app or /usr/local.`
      : `Every ${tool} found is older than the server${server ? ` (${serverLabel(server)})` : ""}.`;
  if (PG_TOOLS.includes(tool)) {
    const formulas: BrewChoice[] = [{ formula: "libpq", label: "libpq — the newest client tools" }];
    // Homebrew keeps the supported majors (14 and later).
    if (server && server.major >= 14)
      formulas.push({
        formula: `postgresql@${server.major}`,
        label: `postgresql@${server.major} — the server's version`,
      });
    return {
      reason,
      formulas,
      download: {
        label: "Download from the PostgreSQL website",
        url: "https://www.postgresql.org/download/macosx/",
      },
    };
  }
  if (server?.flavor === "mariadb")
    return {
      reason,
      formulas: [{ formula: "mariadb", label: "mariadb — MariaDB with its client tools" }],
      download: {
        label: "Download from the MariaDB website",
        url: "https://mariadb.org/download/",
      },
    };
  const formulas: BrewChoice[] = [
    { formula: "mysql-client", label: "mysql-client — the newest client tools" },
  ];
  if (server?.flavor === "mysql" && server.major === 8)
    formulas.push({ formula: "mysql-client@8.4", label: "mysql-client@8.4 — MySQL 8 tools" });
  return {
    reason,
    formulas,
    download: {
      label: "Download from the MySQL website",
      url: "https://dev.mysql.com/downloads/mysql/",
    },
  };
}

/** Opens the download page in the browser. */
export async function openDownloadPage(url: string): Promise<void> {
  if (isMock) {
    window.open(url, "_blank", "noopener");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

/** Runs `brew install <formula>` in a new Terminal window (the user confirms it there). */
export function installWithBrew(formula: string): Promise<void> {
  return ipc.backup.installWithBrew(formula);
}
