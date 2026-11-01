import type { ConnectionProfile } from "@/lib/ipc";
import type { ClickhouseParams } from "@/lib/ipc/bindings/ClickhouseParams";
import type { ClickhouseSetting } from "@/lib/ipc/bindings/ClickhouseSetting";

/**
 * The lane's driver params (`ClickhouseParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own. The form keeps the roles as one
 * comma-separated line and the settings as `name = value` lines.
 */

type Params = Record<string, unknown> | null | undefined;

/** `entry::MAX_SESSION_TIMEOUT_SECS`: the editor's session lasts 1 to 3600 s while idle. */
export const SESSION_TIMEOUT = { min: 1, max: 3600 } as const;

/**
 * `request::RESERVED`: URL parameters the driver sets itself and settings that change what a
 * statement means, where a read goes or how the reply is framed. `params.test.ts` checks this
 * list against the backend's.
 */
export const RESERVED_SETTINGS: readonly string[] = [
  "query",
  "database",
  "default_format",
  "query_id",
  "session_id",
  "session_timeout",
  "session_check",
  "role",
  "user",
  "password",
  "quota_key",
  "readonly",
  "profile",
  "compress",
  "decompress",
  "enable_http_compression",
  "wait_end_of_query",
  "send_progress_in_http_headers",
  "http_write_exception_in_output_format",
  "output_format_write_statistics",
  "output_format",
  "http_x_clickhouse_format_overrides_output_format",
  "framing_output_format",
  "compression",
  "dialect",
  "stream_like_engine_allow_direct_select",
  "implicit_select",
  "implicit_table_at_top_level",
  "select",
  "filter",
  "order",
  "sort",
  "limit",
  "offset",
  "page",
  "format",
  "input_format",
  "additional_result_filter",
  "additional_table_filters",
  "parallel_replicas_custom_key",
  "format_schema_source",
  "format_schema",
  "format_template_resultset",
  "format_template_row",
  "output_format_schema",
  "input_format_record_errors_file_path",
  "http_response_headers",
  "use_query_cache",
  "final",
  "parallel_replicas_count",
  "parallel_replica_offset",
  "url_base",
  "s3_base",
];

/** `request::validate_settings`' name check. */
export const settingNameOk = (name: string): boolean => /^[A-Za-z0-9_]+$/.test(name);

/** `request::validate_settings`' refusal: reserved, `param_*` or `*_overflow_mode(_leaf)`. */
export function settingReserved(name: string): boolean {
  const lower = name.toLowerCase();
  return (
    RESERVED_SETTINGS.includes(lower) ||
    lower.startsWith("param_") ||
    lower.endsWith("_overflow_mode") ||
    lower.endsWith("_overflow_mode_leaf")
  );
}

/** The backend's refusal of a setting, in its words; null when the name is accepted. */
export function settingProblem(name: string): string | null {
  const k = name.trim();
  if (!settingNameOk(k)) return `“${k}” is not a ClickHouse setting name`;
  if (settingReserved(k))
    return `“${k}” is set by Tabler or changes how statements are read, so it cannot be a connection setting`;
  return null;
}

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface ClickhouseForm {
  certificateAuth: boolean;
  /** Role names, comma-separated. */
  roles: string;
  /** One `name = value` per line. */
  settings: string;
  /** Digits as typed; "" for the server's 60 s. */
  sessionTimeoutSecs: string;
}

/** A settings line that is not `name = value`. */
export interface SettingsLineError {
  line: number;
  text: string;
}

/** The settings of the form's text, and the lines that are no setting. Blank lines are skipped. */
export function parseSettings(text: string): {
  settings: ClickhouseSetting[];
  bad: SettingsLineError[];
} {
  const settings: ClickhouseSetting[] = [];
  const bad: SettingsLineError[] = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (line === "") return;
    const eq = line.indexOf("=");
    const name = eq < 0 ? "" : line.slice(0, eq).trim();
    if (name === "") bad.push({ line: i + 1, text: line });
    else settings.push({ name, value: line.slice(eq + 1).trim() });
  });
  return { settings, bad };
}

/** `name = value` lines of settings. */
export const formatSettings = (settings: readonly ClickhouseSetting[]): string =>
  settings.map((s) => `${s.name} = ${s.value}`).join("\n");

/** The role names of the form's line: split at commas, trimmed, blanks dropped. */
export const parseRoles = (text: string): string[] =>
  text
    .split(",")
    .map((r) => r.trim())
    .filter((r) => r !== "");

const digits = (v: unknown): string =>
  typeof v === "number" && Number.isInteger(v) ? String(v) : typeof v === "string" ? v.trim() : "";

/** The seconds of a field, when they are whole and within the range. */
export function sessionTimeoutOf(text: string): number | null {
  if (!/^\d+$/.test(text)) return null;
  const n = Number(text);
  return n >= SESSION_TIMEOUT.min && n <= SESSION_TIMEOUT.max ? n : null;
}

const settingsOf = (v: unknown): ClickhouseSetting[] =>
  Array.isArray(v)
    ? v.flatMap((s: unknown) => {
        if (typeof s !== "object" || s === null) return [];
        const { name, value } = s as Record<string, unknown>;
        return typeof name === "string" && typeof value === "string" ? [{ name, value }] : [];
      })
    : [];

export function clickhouseParamsOf(params: Params): ClickhouseForm {
  const roles = params?.roles;
  const settings = params?.settings;
  return {
    certificateAuth: params?.certificateAuth === true,
    roles: Array.isArray(roles)
      ? roles.filter((r): r is string => typeof r === "string").join(", ")
      : typeof roles === "string"
        ? roles.trim()
        : "",
    settings: typeof settings === "string" ? settings : formatSettings(settingsOf(settings)),
    sessionTimeoutSecs: digits(params?.sessionTimeoutSecs),
  };
}

export const clickhouseParams = (p: ConnectionProfile): ClickhouseForm =>
  clickhouseParamsOf(p.options.driverParams);

/**
 * The params a profile keeps: only what differs from the backend's defaults, so a profile with
 * none stores none. Lines that are no setting are dropped (the form refuses them first).
 */
export function toClickhouseParams(f: ClickhouseForm): Partial<ClickhouseParams> | null {
  const out: Partial<ClickhouseParams> = {};
  if (f.certificateAuth) out.certificateAuth = true;
  const roles = parseRoles(f.roles);
  if (roles.length > 0) out.roles = roles;
  const { settings } = parseSettings(f.settings);
  if (settings.length > 0) out.settings = settings;
  const secs = sessionTimeoutOf(f.sessionTimeoutSecs);
  if (secs !== null) out.sessionTimeoutSecs = secs;
  return Object.keys(out).length > 0 ? out : null;
}
