import { zodResolver } from "@hookform/resolvers/zod";
import {
  Check,
  ChevronDown,
  ChevronRight,
  FilePlus2,
  FolderOpen,
  Info,
  Link2,
  Loader2,
  Network,
  ScrollText,
  ShieldOff,
  Tags,
  X,
} from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm, type FieldErrors, type UseFormReturn } from "react-hook-form";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { rememberSafeModeLevel } from "@/features/safety/guard";
import { SAFE_MODE_DESCRIPTION, SAFE_MODE_SUMMARY } from "@/features/safety/levels";
import { confirmProtectedChange, protectedChanges } from "@/features/safety/setLevel";
import {
  ipc,
  isMock,
  toIpcError,
  type ConnectionProfile,
  type Driver,
  type MysqlVersion,
  type PasswordMode,
  type SafeModeLevel,
  type SshPasswordMode,
  type SslMode,
  type SslNegotiation,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { SECRET_STORE } from "@/lib/platform";
import { ACCENT_PRESETS } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { getPrefs } from "@/stores/prefs";
import { DriverBadge } from "./badges";
import { BootstrapCommandsDialog } from "./BootstrapCommandsDialog";
import {
  accentColor,
  cssColor,
  DATABASE_KINDS,
  databaseKind,
  DRIVER_LABEL,
  DRIVERS,
  KIND_LABEL,
  KIND_TINT,
  kindDriver,
  type DatabaseKind,
} from "./colors";
import { CustomColorSwatch } from "./CustomColorSwatch";
import { EditTagsDialog } from "./EditTagsDialog";
import { GroupCombobox } from "./GroupCombobox";
import { knownGroups, useStoredGroups } from "./groups";
import { openConnectionOf, useOpenProfile } from "./openProfile";
import {
  askConnectionPassword,
  handOverPassword,
  isPromptCancelled,
  requestPassword,
} from "./passwordPromptStore";
import {
  CONNECTION_TAGS,
  connectionFormSchema,
  DEFAULT_CONNECT_TIMEOUT_SECS,
  ENCODING_CHOICES,
  DEFAULT_FORM,
  formToProfileInput,
  MYSQL_REMOTE_SOCKET_PLACEHOLDER,
  MYSQL_SOCKET_PLACEHOLDER,
  MYSQL_VERSION_LABEL,
  MYSQL_VERSIONS,
  newProfileForm,
  PASSWORD_MODE_LABEL,
  passwordModesFor,
  profileToForm,
  SAFE_MODE_LABEL,
  SAFE_MODES,
  SQLCIPHER_COMPATIBILITY,
  SSH_PASSWORD_MODES,
  urlToForm,
  type ConnectionFormValues,
} from "./schema";
import { openConnectionUrlDialog } from "./store";
import { tagColorOf, tagLabel, useCustomTags, type CustomTag } from "./tags";
import type { ParsedConnectionUrl } from "./url";
import {
  hostKeyDeclined,
  useProfiles,
  useSaveProfile,
  useSecretStatus,
  useTestConnection,
} from "./useConnections";

type Form = UseFormReturn<ConnectionFormValues>;

const DEFAULT_PORT: Record<Driver, string> = { postgres: "5432", mysql: "3306", sqlite: "" };

/**
 * SSL modes per driver, in menu order: libpq's names for PostgreSQL, MySQL's for MySQL, and the
 * MariaDB / old MySQL driver's three (Ver. ▸ MariaDB or Old MySQL).
 */
const SSL_MODES: Record<Exclude<Driver, "sqlite"> | "mariadb", ReadonlyArray<[SslMode, string]>> = {
  postgres: [
    ["disable", "disable"],
    ["prefer", "prefer"],
    ["require", "require"],
    ["verify-ca", "verify-ca"],
    ["verify-full", "verify-full"],
  ],
  mysql: [
    ["prefer", "PREFERRED"],
    ["disable", "DISABLED"],
    ["require", "REQUIRED"],
    ["verify-ca", "VERIFY-CA"],
    ["verify-full", "VERIFY-IDENTITY"],
  ],
  mariadb: [
    ["prefer", "PREFERRED"],
    ["require", "ENFORCE"],
    ["verify-full", "VERIFY-SERVER-CERT"],
  ],
};

/** MariaDB, SingleStore DB and Old MySQL share the short list (TablePlus's Ver. menu). */
const SHORT_SSL_LIST: readonly MysqlVersion[] = ["mariadb", "singlestore", "old"];

/** The SSL mode list of the form's driver and MySQL version. */
function sslModesOf(driver: Driver, version: MysqlVersion): ReadonlyArray<[SslMode, string]> {
  if (driver !== "mysql") return SSL_MODES.postgres;
  return SHORT_SSL_LIST.includes(version) ? SSL_MODES.mariadb : SSL_MODES.mysql;
}

/** The MariaDB / SingleStore / old MySQL list has no DISABLED and no VERIFY-CA: their nearest mode. */
function sslForVersion(ssl: SslMode, version: MysqlVersion): SslMode {
  if (!SHORT_SSL_LIST.includes(version)) return ssl;
  return ssl === "disable" ? "prefer" : ssl === "verify-ca" ? "verify-full" : ssl;
}

/**
 * What Old MySQL changes for TLS: servers of the 5.5 / 5.6 era offer TLS 1.0 / 1.1 only, which
 * the default driver (TLS 1.2 and up) cannot speak. The old driver goes through the system's TLS
 * library, which does, but it cannot send the PEM client certificate.
 */
const OLD_MYSQL_TLS_NOTE =
  "Old MySQL: TLS 1.0 and 1.1 servers are supported (macOS), as are pre-4.1 password hashes. Key… / Cert… are only sent over TLS 1.2 or newer";

const SSL_NEGOTIATION_LABEL: Record<SslNegotiation, string> = {
  postgres: "Postgres",
  direct: "Direct",
};

const SSH_PASSWORD_MODE_LABEL: Record<SshPasswordMode, string> = {
  keychain: "Store in keychain",
  ask: "Ask every time",
  none: "No password",
};

/** "Edit Tags…" at the end of the Tag menu (not a tag). */
const EDIT_TAGS = "__tabler.edit-tags__";

const SQLCIPHER_LABEL: Record<(typeof SQLCIPHER_COMPATIBILITY)[number], string> = {
  "": "Default (SQLCipher 4)",
  "3": "SQLCipher 3",
  "2": "SQLCipher 2",
  "1": "SQLCipher 1",
};

const PASSWORD_HINT: Record<Exclude<PasswordMode, "keychain">, string> = {
  ask: "Asked on every connect; kept in memory while connected",
  none: "Connects without a password",
  command: "The output of the command below, run on every connect; never stored",
};

const PASSWORD_PLACEHOLDER: Record<Exclude<PasswordMode, "keychain">, string> = {
  ask: "Asked when connecting",
  none: "No password",
  command: "From the command",
};

const IAM_COMMAND_EXAMPLE: Record<Exclude<Driver, "sqlite">, string> = {
  postgres:
    "aws rds generate-db-auth-token --hostname mydb.abc.us-east-1.rds.amazonaws.com --port 5432 --username app",
  mysql:
    "aws rds generate-db-auth-token --hostname mydb.abc.us-east-1.rds.amazonaws.com --port 3306 --username app",
};

const baseName = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;

/** Label column + field column, TablePlus's connection form layout. */
const GRID = "grid grid-cols-[104px_minmax(0,1fr)] items-start gap-x-3 gap-y-2.5";

/** One line of the form: the label on the left (right-aligned), the field with its message. */
function Row({
  label,
  htmlFor,
  error,
  hint,
  children,
}: {
  label: string;
  htmlFor?: string;
  error?: string;
  hint?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <>
      <Label
        htmlFor={htmlFor}
        className="text-muted-foreground h-8 justify-end text-right text-xs font-normal"
      >
        {label}
      </Label>
      <div className="flex min-w-0 flex-col gap-1">
        {children}
        {error ? (
          <p className="text-error text-[11px]">{error}</p>
        ) : hint ? (
          <p className="text-muted-foreground text-[11px]">{hint}</p>
        ) : null}
      </div>
    </>
  );
}

/** An inline block of the form (SSH, Advanced) with a hairline heading. */
function Section({
  title,
  children,
  sectionRef,
}: {
  title: string;
  children: React.ReactNode;
  sectionRef?: React.Ref<HTMLElement>;
}) {
  return (
    <section ref={sectionRef} aria-label={title} className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h3 className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
          {title}
        </h3>
        <div className="bg-border h-px flex-1" />
      </div>
      <div className={GRID}>{children}</div>
    </section>
  );
}

const SQLITE_FILTERS = [{ name: "SQLite", extensions: ["sqlite", "sqlite3", "db", "db3", "s3db"] }];

async function pickSqliteFile(): Promise<string | null> {
  if (isMock) return window.prompt("Path to SQLite file", "/Users/me/data.sqlite");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    multiple: false,
    directory: false,
    title: "Open SQLite database",
    filters: SQLITE_FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/** Asks where to create a new SQLite database file (save panel). */
async function pickNewSqliteFile(): Promise<string | null> {
  if (isMock) return window.prompt("Create SQLite file at", "/Users/me/database.sqlite");
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({
    title: "New SQLite database",
    defaultPath: "database.sqlite",
    filters: SQLITE_FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/**
 * "New…": creates the database file, then puts its path in the form. Connecting never creates
 * files, so this is the only way to start from an empty database. An existing file chosen in the
 * save panel is left as it is (never truncated).
 */
async function createSqliteFile(form: Form): Promise<void> {
  const path = (await pickNewSqliteFile())?.trim();
  if (!path) return;
  const { passwordMode, password, sqliteCipherCompatibility } = form.getValues();
  // With an encryption key the new database is encrypted (SQLCipher): the key typed in the form,
  // or, when it is asked every time, asked now.
  let key: string | null = passwordMode === "keychain" && password !== "" ? password : null;
  if (passwordMode === "ask") {
    key = await requestPassword({
      title: "Encryption key of the new database",
      description:
        "The new database is encrypted with this key (SQLCipher). It is asked again on every connect.",
      confirmLabel: "Create",
    });
    if (key === null) return;
  }
  const compatibility = sqliteCipherCompatibility === "" ? null : Number(sqliteCipherCompatibility);
  try {
    await ipc.profiles.createSqliteFile(path, key || null, compatibility);
  } catch (err) {
    toast.error("Could not create the SQLite file", { description: toIpcError(err).message });
    return;
  }
  form.setValue("sqlitePath", path, { shouldValidate: true, shouldDirty: true });
}

async function pickFile(title: string): Promise<string | null> {
  if (isMock) return window.prompt(title, "");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, directory: false, title });
  return typeof picked === "string" ? picked : null;
}

type SslFileField = "sslKeyPath" | "sslCertPath" | "sslCaPath";

/** TablePlus's "Key…" / "Cert…" / "CA Cert…" buttons: the chosen file's name, and "—" to clear. */
function SslFileButton({
  form,
  field,
  label,
  title,
}: {
  form: Form;
  field: SslFileField;
  label: string;
  title: string;
}) {
  const path = form.watch(field);
  const set = (v: string) =>
    form.setValue(field, v, { shouldDirty: true, shouldValidate: form.formState.isSubmitted });
  return (
    <div className="flex items-center">
      <Button
        type="button"
        variant="outline"
        size="sm"
        title={path ? path : title}
        aria-label={path ? `${label} ${baseName(path)}` : label}
        className={cn("max-w-40", path && "rounded-r-none font-mono text-[11px]")}
        onClick={async () => {
          const p = await pickFile(title);
          if (p) set(p);
        }}
      >
        <span className="truncate">{path ? baseName(path) : label}</span>
      </Button>
      {path && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-label={`Clear ${label.replace("…", "")}`}
          title="Remove"
          className="-ml-px rounded-l-none px-2"
          onClick={() => set("")}
        >
          —
        </Button>
      )}
    </div>
  );
}

/**
 * New connection, step 1: one big tile per database (TablePlus's list; MariaDB apart from MySQL). A click selects, a double-click or ↩ (or the
 * dialog's Create button) goes on to the form; ←/→ move between the tiles.
 */
function DriverPicker({
  value,
  onChange,
  onConfirm,
}: {
  value: DatabaseKind;
  onChange: (kind: DatabaseKind) => void;
  onConfirm: (kind: DatabaseKind) => void;
}) {
  const tiles = useRef(new Map<DatabaseKind, HTMLButtonElement>());
  return (
    <div role="radiogroup" aria-label="Database" className="grid grid-cols-4 gap-3">
      {DATABASE_KINDS.map((d, i) => (
        <button
          key={d}
          ref={(el) => {
            if (el) tiles.current.set(d, el);
            else tiles.current.delete(d);
          }}
          type="button"
          role="radio"
          aria-checked={value === d}
          tabIndex={value === d ? 0 : -1}
          onClick={() => onChange(d)}
          onDoubleClick={() => onConfirm(d)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onConfirm(d);
              return;
            }
            const step =
              e.key === "ArrowRight" || e.key === "ArrowDown"
                ? 1
                : e.key === "ArrowLeft" || e.key === "ArrowUp"
                  ? -1
                  : 0;
            if (step === 0) return;
            e.preventDefault();
            const next =
              DATABASE_KINDS[(i + step + DATABASE_KINDS.length) % DATABASE_KINDS.length]!;
            onChange(next);
            tiles.current.get(next)?.focus();
          }}
          className={cn(
            "flex flex-col items-center gap-3 rounded-xl border px-3 pt-6 pb-5 text-center transition-colors outline-none",
            "focus-visible:ring-ring/50 focus-visible:ring-3",
            value === d
              ? "border-brand bg-brand-soft"
              : "border-border hover:border-border-strong hover:bg-row-hover",
          )}
        >
          <DriverBadge
            driver={kindDriver(d)}
            mysqlVersion={d === "mariadb" ? "mariadb" : null}
            color={KIND_TINT[d]}
            size="lg"
          />
          <span className="text-[13px] font-medium">{KIND_LABEL[d]}</span>
        </button>
      ))}
    </div>
  );
}

/** "Bootstrap commands…" and a one-line preview of what is set. */
function BootstrapRow({ form, onOpen }: { form: Form; onOpen: () => void }) {
  const sql = form.watch("startupSql").trim();
  const script = form.watch("bootstrapScript").trim();
  const lines = sql === "" ? [] : sql.split("\n").filter((l) => l.trim() !== "");
  return (
    <Row label="">
      <div className="flex h-8 min-w-0 items-center gap-2">
        <Button type="button" variant="outline" size="sm" onClick={onOpen}>
          <ScrollText /> Bootstrap commands…
        </Button>
        {lines.length > 0 && (
          <span
            className="text-muted-foreground min-w-0 truncate font-mono text-[11px]"
            title={sql}
            data-testid="bootstrap-summary"
          >
            {lines[0]}
            {lines.length > 1 ? ` (+${lines.length - 1} more)` : ""}
          </span>
        )}
        {script !== "" && (
          <span
            className="text-muted-foreground shrink-0 text-[11px]"
            title={script}
            data-testid="bootstrap-script-summary"
          >
            {lines.length > 0 ? "· " : ""}+ shell script
          </span>
        )}
      </div>
    </Row>
  );
}

/** The Tag menu: the built-ins, the custom tags, and the form's tag when nobody defines it. */
function tagOptions(current: string | null, custom: readonly CustomTag[]): string[] {
  const out: string[] = [...CONNECTION_TAGS, ...custom.map((t) => t.name)];
  if (current && !out.some((t) => t.toLowerCase() === current.toLowerCase())) out.push(current);
  return out;
}

function GeneralFields({
  form,
  hasStoredPassword,
  groups,
  onBootstrap,
  onEditTags,
}: {
  form: Form;
  hasStoredPassword: boolean;
  /** Existing group names, suggested in the Group field. */
  groups: readonly string[];
  onBootstrap: () => void;
  onEditTags: () => void;
}) {
  const { register, control, formState, watch, setValue, getValues } = form;
  const customTags = useCustomTags();
  const mysqlVersion = watch("mysqlVersion");
  const errors = formState.errors;
  const uid = useId();
  const driver = watch("driver");
  const clearPassword = watch("clearPassword");
  const passwordMode = watch("passwordMode");
  const ssl = watch("ssl");
  const sqlite = driver === "sqlite";
  const keychain = passwordMode === "keychain";
  const socket = driver === "mysql" && watch("useSocket");
  const socketOverSsh = socket && watch("sshEnabled");
  /** MySQL "Use socket": swaps Host / Port for the socket path. */
  const socketToggle = driver === "mysql" && (
    <label className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-xs whitespace-nowrap">
      <Controller
        control={control}
        name="useSocket"
        render={({ field }) => (
          <Checkbox
            checked={field.value}
            onCheckedChange={(v) => field.onChange(v === true)}
            aria-label="Use socket"
          />
        )}
      />
      Use socket
    </label>
  );
  const sslKeys =
    ssl !== "disable" ||
    [watch("sslKeyPath"), watch("sslCertPath"), watch("sslCaPath")].some((p) => p.trim() !== "");
  return (
    <div className={GRID}>
      <Row label="Name" htmlFor={`${uid}-name`} error={errors.name?.message}>
        <Input
          id={`${uid}-name`}
          {...register("name")}
          placeholder="My database"
          aria-invalid={errors.name ? true : undefined}
          autoFocus
        />
      </Row>
      <Row label="Tag" htmlFor={`${uid}-tag`}>
        <div className="flex h-8 items-center gap-3">
          <Controller
            control={control}
            name="tag"
            render={({ field }) => (
              <Select
                value={field.value ?? ""}
                onValueChange={(v) => {
                  // "Edit Tags…" opens the tag editor and keeps the current tag.
                  if (v === EDIT_TAGS) onEditTags();
                  else field.onChange(v);
                }}
              >
                <SelectTrigger id={`${uid}-tag`} className="w-36">
                  <SelectValue placeholder="None" />
                </SelectTrigger>
                <SelectContent>
                  {tagOptions(field.value, customTags).map((t) => (
                    <SelectItem key={t} value={t}>
                      <span
                        aria-hidden
                        className="size-2 shrink-0 rounded-full"
                        style={{ background: cssColor(tagColorOf(t, customTags)) }}
                      />
                      <span className="truncate">{tagLabel(t)}</span>
                    </SelectItem>
                  ))}
                  <SelectSeparator />
                  <SelectItem value={EDIT_TAGS}>
                    <Tags aria-hidden className="size-3.5" />
                    Edit Tags…
                  </SelectItem>
                </SelectContent>
              </Select>
            )}
          />
          <span className="text-muted-foreground text-xs">Status color</span>
          <Controller
            control={control}
            name="color"
            render={({ field }) => (
              <div
                role="radiogroup"
                aria-label="Status color"
                className="flex items-center gap-1.5"
              >
                {ACCENT_PRESETS.map((c) => (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={field.value === c}
                    aria-label={c}
                    title={c}
                    onClick={() => field.onChange(c)}
                    className={cn(
                      "ring-offset-background size-4 rounded-full ring-offset-2 transition-transform outline-none",
                      "focus-visible:ring-ring focus-visible:ring-2",
                      field.value === c ? "ring-foreground/60 scale-110 ring-2" : "hover:scale-110",
                    )}
                    style={{ background: accentColor(c) }}
                  />
                ))}
                <CustomColorSwatch value={field.value} onChange={field.onChange} />
              </div>
            )}
          />
        </div>
      </Row>
      <Row label="Group" htmlFor={`${uid}-group`} error={errors.group?.message}>
        <Controller
          control={control}
          name="group"
          render={({ field }) => (
            <GroupCombobox
              id={`${uid}-group`}
              value={field.value}
              onChange={field.onChange}
              onBlur={field.onBlur}
              groups={groups}
              placeholder="Optional, e.g. Clients / Acme"
              invalid={errors.group !== undefined}
            />
          )}
        />
      </Row>
      <Row label="Driver" htmlFor={`${uid}-driver`}>
        <div className="flex items-center gap-2">
          <Controller
            control={control}
            name="driver"
            render={({ field }) => (
              <Select
                value={field.value}
                onValueChange={(v) => {
                  const next = v as Driver;
                  const port = getValues("port").trim();
                  field.onChange(next);
                  // Follow the driver unless the port was typed in.
                  if (next !== "sqlite" && (port === "" || port === "5432" || port === "3306"))
                    setValue("port", DEFAULT_PORT[next]);
                }}
              >
                <SelectTrigger id={`${uid}-driver`} className="w-48">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {DRIVERS.map((d) => (
                    <SelectItem key={d} value={d}>
                      {DRIVER_LABEL[d]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {driver === "mysql" && (
            <>
              <Label
                htmlFor={`${uid}-version`}
                className="text-muted-foreground shrink-0 text-xs font-normal"
              >
                Ver.
              </Label>
              <Controller
                control={control}
                name="mysqlVersion"
                render={({ field }) => (
                  <Select
                    value={field.value}
                    onValueChange={(v) => {
                      const version = v as MysqlVersion;
                      field.onChange(version);
                      // The MariaDB / old MySQL list has its own three SSL modes.
                      setValue("ssl", sslForVersion(getValues("ssl"), version), {
                        shouldDirty: true,
                      });
                    }}
                  >
                    <SelectTrigger id={`${uid}-version`} className="w-40">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {MYSQL_VERSIONS.map((m) => (
                        <SelectItem key={m} value={m}>
                          {MYSQL_VERSION_LABEL[m]}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </>
          )}
        </div>
      </Row>

      {sqlite ? <SqliteFields form={form} hasStoredPassword={hasStoredPassword} /> : null}
      {sqlite ? (
        <BootstrapRow form={form} onOpen={onBootstrap} />
      ) : (
        <>
          {socket ? (
            <Row
              label="Socket"
              htmlFor={`${uid}-socket`}
              error={errors.socketPath?.message}
              hint={
                socketOverSsh
                  ? "The socket file on the SSH server, forwarded through the tunnel. Leave empty to try /var/run/mysqld/mysqld.sock, /tmp/mysql.sock and /var/lib/mysql/mysql.sock there"
                  : "Leave empty to detect it (/tmp/mysql.sock, /var/run/mysqld/mysqld.sock, Homebrew's)"
              }
            >
              <div className="flex items-center gap-2">
                <Input
                  id={`${uid}-socket`}
                  {...register("socketPath")}
                  placeholder={
                    socketOverSsh ? MYSQL_REMOTE_SOCKET_PLACEHOLDER : MYSQL_SOCKET_PLACEHOLDER
                  }
                  className="font-mono"
                  aria-invalid={errors.socketPath ? true : undefined}
                />
                {socketToggle}
              </div>
            </Row>
          ) : (
            <Row
              label="Host"
              htmlFor={`${uid}-host`}
              error={errors.host?.message ?? errors.port?.message}
            >
              <div className="flex items-center gap-2">
                <Input
                  id={`${uid}-host`}
                  {...register("host")}
                  placeholder="127.0.0.1"
                  className="font-mono"
                  aria-invalid={errors.host ? true : undefined}
                />
                <Label
                  htmlFor={`${uid}-port`}
                  className="text-muted-foreground shrink-0 text-xs font-normal"
                >
                  Port
                </Label>
                <Input
                  id={`${uid}-port`}
                  {...register("port")}
                  placeholder={DEFAULT_PORT[driver]}
                  className="w-20 shrink-0 font-mono"
                  inputMode="numeric"
                  aria-invalid={errors.port ? true : undefined}
                />
                {socketToggle}
              </div>
            </Row>
          )}
          <Row label="User" htmlFor={`${uid}-user`} error={errors.user?.message}>
            <Input
              id={`${uid}-user`}
              {...register("user")}
              placeholder={driver === "postgres" ? "postgres" : "root"}
              className="font-mono"
              autoComplete="off"
              aria-invalid={errors.user ? true : undefined}
            />
          </Row>
          <Row
            label="Password"
            htmlFor={`${uid}-password`}
            hint={
              !keychain
                ? PASSWORD_HINT[passwordMode]
                : hasStoredPassword && !clearPassword
                  ? "Stored in Keychain — leave empty to keep"
                  : clearPassword
                    ? "The stored password is removed on save"
                    : `Stored in the ${SECRET_STORE}`
            }
          >
            <div className="flex items-center gap-2">
              <Input
                id={`${uid}-password`}
                {...register("password")}
                type="password"
                placeholder={
                  !keychain
                    ? PASSWORD_PLACEHOLDER[passwordMode]
                    : hasStoredPassword && !clearPassword
                      ? "••••••••"
                      : ""
                }
                className="font-mono"
                autoComplete="new-password"
                disabled={clearPassword || !keychain}
              />
              <Controller
                control={control}
                name="passwordMode"
                render={({ field }) => (
                  <Select
                    value={field.value}
                    onValueChange={(v) => {
                      field.onChange(v as PasswordMode);
                      // Only the Keychain keeps a typed password.
                      if (v !== "keychain") setValue("password", "");
                    }}
                  >
                    <SelectTrigger aria-label="Password storage" className="w-40 shrink-0">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {passwordModesFor("postgres").map((m) => (
                        <SelectItem key={m} value={m}>
                          {PASSWORD_MODE_LABEL[m]}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
              {hasStoredPassword && keychain && (
                <label
                  className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-[11px] whitespace-nowrap"
                  title="Remove the password stored in the Keychain"
                >
                  <Controller
                    control={control}
                    name="clearPassword"
                    render={({ field }) => (
                      <Checkbox
                        checked={field.value}
                        onCheckedChange={(v) => field.onChange(v === true)}
                      />
                    )}
                  />
                  Clear
                </label>
              )}
            </div>
          </Row>
          {passwordMode === "command" && (
            <Row
              label="Command"
              htmlFor={`${uid}-command`}
              error={errors.passwordCommand?.message}
              hint={
                <>
                  Runs in your login shell (<span className="font-mono">$SHELL -lc</span>, 30 s
                  limit); its output is the password
                </>
              }
            >
              <Textarea
                id={`${uid}-command`}
                {...register("passwordCommand")}
                placeholder={IAM_COMMAND_EXAMPLE[driver]}
                rows={2}
                spellCheck={false}
                autoComplete="off"
                className="min-h-0 resize-none font-mono text-xs"
                aria-invalid={errors.passwordCommand ? true : undefined}
              />
            </Row>
          )}
          <Row
            label="Database"
            htmlFor={`${uid}-database`}
            hint={driver === "postgres" ? "Defaults to postgres" : "Optional"}
          >
            <Input id={`${uid}-database`} {...register("database")} className="font-mono" />
          </Row>
          <BootstrapRow form={form} onOpen={onBootstrap} />
          <Row
            label="SSL mode"
            htmlFor={`${uid}-ssl`}
            error={errors.pgSslNegotiation?.message}
            hint={
              driver === "mysql" && ssl === "verify-ca"
                ? "Checks the certificate chain (the CA file, else the system's roots) but not the host name; VERIFY-IDENTITY checks the name too"
                : driver === "mysql" && mysqlVersion === "old"
                  ? OLD_MYSQL_TLS_NOTE
                  : driver === "postgres" && watch("pgSslNegotiation") === "direct"
                    ? "Direct: TLS right away, without asking the server first (PostgreSQL 17+)"
                    : undefined
            }
          >
            <div className="flex items-center gap-2">
              <Controller
                control={control}
                name="ssl"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id={`${uid}-ssl`} className="w-48">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {sslModesOf(driver, mysqlVersion).map(([m, label]) => (
                        <SelectItem key={m} value={m}>
                          {label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
              {driver === "postgres" && (
                <>
                  <Label
                    htmlFor={`${uid}-negotiation`}
                    className="text-muted-foreground shrink-0 text-xs font-normal"
                  >
                    Negotiation
                  </Label>
                  <Controller
                    control={control}
                    name="pgSslNegotiation"
                    render={({ field }) => (
                      <Select
                        value={field.value}
                        onValueChange={(v) => field.onChange(v as SslNegotiation)}
                      >
                        <SelectTrigger
                          id={`${uid}-negotiation`}
                          aria-label="SSL negotiation"
                          className="w-28"
                          aria-invalid={errors.pgSslNegotiation ? true : undefined}
                        >
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {(["postgres", "direct"] as const).map((m) => (
                            <SelectItem key={m} value={m}>
                              {SSL_NEGOTIATION_LABEL[m]}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    )}
                  />
                </>
              )}
            </div>
          </Row>
          {sslKeys && (
            <Row
              label="SSL keys"
              error={errors.sslKeyPath?.message ?? errors.sslCertPath?.message}
              hint="PEM files: an unencrypted key (PKCS#8, PKCS#1 or SEC1) and the client certificate followed by its intermediates (one file with both works for Key… and Cert…); the CA certificate for verify-ca / verify-full"
            >
              <div
                role="group"
                aria-label="SSL keys"
                className="flex min-h-8 flex-wrap items-center gap-1.5"
              >
                <SslFileButton
                  form={form}
                  field="sslKeyPath"
                  label="Key…"
                  title="Choose the client private key (PEM)"
                />
                <SslFileButton
                  form={form}
                  field="sslCertPath"
                  label="Cert…"
                  title="Choose the client certificate (PEM)"
                />
                <SslFileButton
                  form={form}
                  field="sslCaPath"
                  label="CA Cert…"
                  title="Choose the CA certificate (PEM)"
                />
              </div>
            </Row>
          )}
          <OtherOptionsRow form={form} />
        </>
      )}
    </div>
  );
}

/** The driver toggles of TablePlus's "Other options" menu, as [field, label, hint]. */
const OTHER_OPTIONS: Record<
  Exclude<Driver, "sqlite">,
  ReadonlyArray<
    readonly [
      "mysqlCleartextPlugin" | "mysqlLocalInfile" | "pgDisableChannelBinding",
      string,
      string,
    ]
  >
> = {
  mysql: [
    [
      "mysqlCleartextPlugin",
      "Enable CLEARTEXT_PLUGIN",
      "Sends the password in clear text when the server asks (PAM, LDAP); over SSL or a socket only",
    ],
    [
      "mysqlLocalInfile",
      "Enable LOAD DATA LOCAL INFILE",
      "The server may read local files; Tabler asks before it sends each one",
    ],
  ],
  postgres: [
    [
      "pgDisableChannelBinding",
      "Disable channel binding",
      "Logs in with SCRAM-SHA-256 without channel binding (some poolers and proxies need it)",
    ],
  ],
};

/** "Other options": a menu of the driver's toggles, with the ones that are on listed beside it. */
function OtherOptionsRow({ form }: { form: Form }) {
  const driver = form.watch("driver");
  const flags = {
    mysqlCleartextPlugin: form.watch("mysqlCleartextPlugin"),
    mysqlLocalInfile: form.watch("mysqlLocalInfile"),
    pgDisableChannelBinding: form.watch("pgDisableChannelBinding"),
  };
  if (driver === "sqlite") return null;
  const options = OTHER_OPTIONS[driver];
  const on = options.filter(([field]) => flags[field]);
  return (
    <Row
      label="Other options"
      hint={on.length > 0 ? on.map(([, , hint]) => hint).join(" · ") : undefined}
    >
      <div className="flex h-8 min-w-0 items-center gap-2">
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm">
              Other options
              <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {options.map(([field, label]) => (
              <DropdownMenuCheckboxItem
                key={field}
                checked={flags[field]}
                onCheckedChange={(v) => form.setValue(field, v === true, { shouldDirty: true })}
              >
                {label}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        {on.length > 0 && (
          <span
            className="text-muted-foreground min-w-0 truncate text-[11px]"
            data-testid="other-options-summary"
          >
            {on.map(([, label]) => label).join(", ")}
          </span>
        )}
      </div>
    </Row>
  );
}

/**
 * The SQLite part of the form: the database file (or its path on the SSH server, for SQLite over
 * SSH), the SQLCipher encryption key with its storage mode and compatibility, the `.sqliterc`
 * resource file, and the upload switch of SQLite over SSH.
 */
function SqliteFields({ form, hasStoredPassword }: { form: Form; hasStoredPassword: boolean }) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const uid = useId();
  const remote = watch("sshEnabled");
  const mode = watch("passwordMode");
  const clearPassword = watch("clearPassword");
  const keychain = mode === "keychain";
  const resourceFile = watch("sqliteResourceFile");
  return (
    <>
      {remote ? (
        <Row
          label="Remote path"
          htmlFor={`${uid}-file`}
          error={errors.sqlitePath?.message}
          hint="The file is downloaded over SFTP on connect (fine for small databases); without upload it opens read-only."
        >
          <Input
            id={`${uid}-file`}
            {...register("sqlitePath")}
            placeholder="/var/lib/app/database.sqlite"
            className="font-mono"
            aria-invalid={errors.sqlitePath ? true : undefined}
          />
        </Row>
      ) : (
        <Row label="Database file" htmlFor={`${uid}-file`} error={errors.sqlitePath?.message}>
          <div className="flex gap-2">
            <Input
              id={`${uid}-file`}
              {...register("sqlitePath")}
              placeholder="/path/to/database.sqlite"
              className="font-mono"
              aria-invalid={errors.sqlitePath ? true : undefined}
            />
            <Button
              type="button"
              variant="outline"
              title="Open an existing database file"
              onClick={async () => {
                const p = await pickSqliteFile();
                if (p) setValue("sqlitePath", p, { shouldValidate: true, shouldDirty: true });
              }}
            >
              <FolderOpen /> Choose…
            </Button>
            <Button
              type="button"
              variant="outline"
              title="Create a new, empty database file (encrypted with the key below, if any)"
              onClick={() => void createSqliteFile(form)}
            >
              <FilePlus2 /> New…
            </Button>
          </div>
        </Row>
      )}
      {remote && (
        <Row label="">
          <label className="text-muted-foreground flex h-8 items-center gap-2 text-xs">
            <Controller
              control={control}
              name="sqliteSshUpload"
              render={({ field }) => (
                <Checkbox
                  checked={field.value}
                  onCheckedChange={(v) => field.onChange(v === true)}
                  aria-label="Upload changes back on Commit / Disconnect"
                />
              )}
            />
            Upload changes back on Commit / Disconnect
          </label>
        </Row>
      )}
      <Row
        label="Encryption key"
        htmlFor={`${uid}-key`}
        hint={
          mode === "ask"
            ? "Asked on every connect; kept in memory while connected"
            : mode === "none"
              ? "A plain, unencrypted database"
              : hasStoredPassword && !clearPassword
                ? "Stored in Keychain — leave empty to keep"
                : clearPassword
                  ? "The stored key is removed on save"
                  : "SQLCipher; leave empty for a plain database"
        }
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-key`}
            {...register("password")}
            type="password"
            placeholder={
              !keychain
                ? mode === "ask"
                  ? "Asked when connecting"
                  : "No key"
                : hasStoredPassword && !clearPassword
                  ? "••••••••"
                  : ""
            }
            className="font-mono"
            autoComplete="new-password"
            disabled={clearPassword || !keychain}
          />
          <Controller
            control={control}
            name="passwordMode"
            render={({ field }) => (
              <Select
                value={field.value}
                onValueChange={(v) => {
                  field.onChange(v as PasswordMode);
                  if (v !== "keychain") setValue("password", "");
                }}
              >
                <SelectTrigger aria-label="Encryption key storage" className="w-40 shrink-0">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {passwordModesFor("sqlite").map((m) => (
                    <SelectItem key={m} value={m}>
                      {m === "none" ? "No key" : PASSWORD_MODE_LABEL[m]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
          {hasStoredPassword && keychain && (
            <label
              className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-[11px] whitespace-nowrap"
              title="Remove the key stored in the Keychain"
            >
              <Controller
                control={control}
                name="clearPassword"
                render={({ field }) => (
                  <Checkbox
                    checked={field.value}
                    onCheckedChange={(v) => field.onChange(v === true)}
                  />
                )}
              />
              Clear
            </label>
          )}
        </div>
      </Row>
      {mode !== "none" && (
        <Row label="Compatibility" htmlFor={`${uid}-cipher`}>
          <Controller
            control={control}
            name="sqliteCipherCompatibility"
            render={({ field }) => (
              <Select
                value={field.value === "" ? "4" : field.value}
                onValueChange={(v) =>
                  field.onChange(
                    (v === "4" ? "" : v) as ConnectionFormValues["sqliteCipherCompatibility"],
                  )
                }
              >
                <SelectTrigger
                  id={`${uid}-cipher`}
                  aria-label="SQLCipher compatibility"
                  className="w-48"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SQLCIPHER_COMPATIBILITY.map((v) => (
                    <SelectItem key={v || "4"} value={v || "4"}>
                      {SQLCIPHER_LABEL[v]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          />
        </Row>
      )}
      <Row
        label="Resource file"
        htmlFor={`${uid}-rc`}
        hint=".sqliterc: its SQL runs after open, before the bootstrap commands (sqlite3 dot-commands are skipped)"
      >
        <div className="flex gap-2">
          <Input
            id={`${uid}-rc`}
            {...register("sqliteResourceFile")}
            placeholder="~/.sqliterc"
            className="font-mono"
          />
          <Button
            type="button"
            variant="outline"
            aria-label="Choose the resource file"
            title="Choose the resource file"
            onClick={async () => {
              const p = await pickFile("Choose the resource file (.sqliterc)");
              if (p) setValue("sqliteResourceFile", p, { shouldDirty: true });
            }}
          >
            <FolderOpen /> Choose…
          </Button>
          {resourceFile !== "" && (
            <Button
              type="button"
              variant="outline"
              aria-label="Clear the resource file"
              title="Remove"
              className="px-2"
              onClick={() => setValue("sqliteResourceFile", "", { shouldDirty: true })}
            >
              —
            </Button>
          )}
        </div>
      </Row>
    </>
  );
}

function SshFields({ form }: { form: Form }) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const uid = useId();
  const auth = watch("sshAuth");
  const passwordMode = watch("sshPasswordMode");
  /** SQLite over SSH downloads the file: no tunnel, so no local port. */
  const tunnel = watch("driver") !== "sqlite";
  return (
    <>
      <Row
        label="SSH host"
        htmlFor={`${uid}-host`}
        error={errors.sshHost?.message ?? errors.sshPort?.message}
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-host`}
            {...register("sshHost")}
            placeholder="bastion.example.com"
            className="font-mono"
            aria-invalid={errors.sshHost ? true : undefined}
          />
          <Label
            htmlFor={`${uid}-port`}
            className="text-muted-foreground shrink-0 text-xs font-normal"
          >
            Port
          </Label>
          <Input
            id={`${uid}-port`}
            {...register("sshPort")}
            placeholder="22"
            className="w-20 shrink-0 font-mono"
            inputMode="numeric"
            aria-invalid={errors.sshPort ? true : undefined}
          />
        </div>
      </Row>
      <Row label="SSH user" htmlFor={`${uid}-user`} error={errors.sshUser?.message}>
        <Input
          id={`${uid}-user`}
          {...register("sshUser")}
          placeholder="Leave empty to use ~/.ssh/config"
          className="font-mono"
          autoComplete="off"
          aria-invalid={errors.sshUser ? true : undefined}
        />
      </Row>
      <Row label="Authentication" htmlFor={`${uid}-auth`}>
        <Controller
          control={control}
          name="sshAuth"
          render={({ field }) => (
            <Select value={field.value} onValueChange={field.onChange}>
              <SelectTrigger id={`${uid}-auth`} className="w-48">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="key">Private key</SelectItem>
                <SelectItem value="password">Password</SelectItem>
                <SelectItem value="agent">SSH agent</SelectItem>
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      {auth === "key" && (
        <>
          <Row label="Private key" htmlFor={`${uid}-key`} error={errors.sshKeyPath?.message}>
            <div className="flex gap-2">
              <Input
                id={`${uid}-key`}
                {...register("sshKeyPath")}
                placeholder="Leave empty to use ~/.ssh/config"
                className="font-mono"
                aria-invalid={errors.sshKeyPath ? true : undefined}
              />
              <Button
                type="button"
                variant="outline"
                onClick={async () => {
                  const p = await pickFile("Choose private key");
                  if (p) setValue("sshKeyPath", p, { shouldValidate: true, shouldDirty: true });
                }}
              >
                <FolderOpen /> Browse…
              </Button>
            </div>
          </Row>
          <Row
            label="Passphrase"
            htmlFor={`${uid}-passphrase`}
            hint="Leave empty if the key is not encrypted"
          >
            <Input
              id={`${uid}-passphrase`}
              {...register("sshPassphrase")}
              type="password"
              className="font-mono"
              autoComplete="new-password"
            />
          </Row>
        </>
      )}
      {auth === "password" && (
        <Row
          label="SSH password"
          htmlFor={`${uid}-password`}
          hint={
            passwordMode === "ask"
              ? "Asked on every connect, in the SSH prompt"
              : passwordMode === "none"
                ? "Sends no password (the server's own questions are still answered)"
                : undefined
          }
        >
          <div className="flex items-center gap-2">
            <Input
              id={`${uid}-password`}
              {...register("sshPassword")}
              type="password"
              className="font-mono"
              autoComplete="new-password"
              placeholder={passwordMode === "keychain" ? "" : SSH_PASSWORD_MODE_LABEL[passwordMode]}
              disabled={passwordMode !== "keychain"}
            />
            <Controller
              control={control}
              name="sshPasswordMode"
              render={({ field }) => (
                <Select
                  value={field.value}
                  onValueChange={(v) => {
                    field.onChange(v as SshPasswordMode);
                    if (v !== "keychain") setValue("sshPassword", "");
                  }}
                >
                  <SelectTrigger aria-label="SSH password storage" className="w-40 shrink-0">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {SSH_PASSWORD_MODES.map((m) => (
                      <SelectItem key={m} value={m}>
                        {SSH_PASSWORD_MODE_LABEL[m]}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>
        </Row>
      )}
      {auth === "agent" && (
        <>
          <span />
          <p className="text-muted-foreground text-[11px]">
            Uses the identities loaded in your SSH agent (<span className="font-mono">ssh-add</span>
            ).
          </p>
        </>
      )}
      <Row
        label={tunnel ? "Local port" : "Keep-alive"}
        htmlFor={tunnel ? `${uid}-local-port` : `${uid}-keepalive`}
        error={errors.sshLocalPort?.message ?? errors.sshKeepaliveSecs?.message}
      >
        <div className="flex items-center gap-2">
          {tunnel && (
            <Input
              id={`${uid}-local-port`}
              {...register("sshLocalPort")}
              placeholder="auto"
              className="w-24 shrink-0 font-mono"
              inputMode="numeric"
              aria-invalid={errors.sshLocalPort ? true : undefined}
            />
          )}
          <Label
            htmlFor={`${uid}-keepalive`}
            className="text-muted-foreground shrink-0 text-xs font-normal"
          >
            Keep-alive interval (s)
          </Label>
          <Input
            id={`${uid}-keepalive`}
            {...register("sshKeepaliveSecs")}
            placeholder="30"
            className="w-20 shrink-0 font-mono"
            inputMode="numeric"
            aria-invalid={errors.sshKeepaliveSecs ? true : undefined}
          />
        </div>
      </Row>
      <span />
      <p className="text-muted-foreground flex items-start gap-1.5 text-[11px]">
        <Info className="mt-px size-3.5 shrink-0" />
        <span className="min-w-0">
          Empty fields use <span className="font-mono">~/.ssh/config</span> · the host key is
          confirmed on first connect
        </span>
      </p>
    </>
  );
}

function AdvancedFields({ form }: { form: Form }) {
  const { register, control, formState, watch } = form;
  const uid = useId();
  const driver = watch("driver");
  const safeMode = watch("safeMode");
  return (
    <>
      <Row label="Safe mode" htmlFor={`${uid}-safe`} hint={SAFE_MODE_DESCRIPTION[safeMode]}>
        <Controller
          control={control}
          name="safeMode"
          render={({ field }) => (
            <Select value={field.value} onValueChange={(v) => field.onChange(v as SafeModeLevel)}>
              <SelectTrigger id={`${uid}-safe`} className="w-80">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SAFE_MODES.map((m) => (
                  <SelectItem key={m} value={m} textValue={SAFE_MODE_LABEL[m]}>
                    {SAFE_MODE_LABEL[m]}{" "}
                    <span className="text-muted-foreground">· {SAFE_MODE_SUMMARY[m]}</span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Row>
      <Row label="Read-only" htmlFor={`${uid}-ro`}>
        <div className="flex h-8 items-center gap-2">
          <Controller
            control={control}
            name="readOnly"
            render={({ field }) => (
              <Switch id={`${uid}-ro`} checked={field.value} onCheckedChange={field.onChange} />
            )}
          />
          <span className="text-muted-foreground text-xs">
            Open the session read-only (no edits, no DDL)
          </span>
        </div>
      </Row>
      {driver === "sqlite" ? (
        <Row label="Foreign keys" htmlFor={`${uid}-fk`}>
          <div className="flex h-8 items-center gap-2">
            <Controller
              control={control}
              name="sqliteForeignKeys"
              render={({ field }) => (
                <Switch id={`${uid}-fk`} checked={field.value} onCheckedChange={field.onChange} />
              )}
            />
            <span className="text-muted-foreground text-xs">
              Enforce them <span className="font-mono text-[11px]">(PRAGMA foreign_keys)</span>;
              SQLite leaves this off unless asked
            </span>
          </div>
        </Row>
      ) : (
        <Row
          label="Time zone"
          htmlFor={`${uid}-tz`}
          hint="e.g. UTC or Asia/Jakarta; empty keeps the server default"
        >
          <Input id={`${uid}-tz`} {...register("timezone")} className="w-56 font-mono" />
        </Row>
      )}
      {driver !== "sqlite" && (
        <Row
          label="Encoding"
          htmlFor={`${uid}-encoding`}
          error={formState.errors.encoding?.message}
          hint={
            driver === "mysql"
              ? "SET NAMES; empty means utf8mb4 (Connection ▸ View Using Encoding… changes it while connected)"
              : "client_encoding; empty means UTF8 (Connection ▸ View Using Encoding… changes it while connected)"
          }
        >
          <Input
            id={`${uid}-encoding`}
            {...register("encoding")}
            list={`${uid}-encodings`}
            placeholder={driver === "mysql" ? "utf8mb4" : "UTF8"}
            className="w-56 font-mono"
            autoComplete="off"
            aria-invalid={formState.errors.encoding ? true : undefined}
          />
          <datalist id={`${uid}-encodings`}>
            {ENCODING_CHOICES[driver].map((e) => (
              <option key={e} value={e} />
            ))}
          </datalist>
        </Row>
      )}
      {driver === "postgres" && (
        <Row
          label="Parameters"
          htmlFor={`${uid}-params`}
          hint={
            <>
              libpq style <span className="font-mono">key=value</span>, separated by spaces or new
              lines: application_name, options, target_session_attrs, or any server setting
              (statement_timeout, search_path, jit…)
            </>
          }
        >
          <Textarea
            id={`${uid}-params`}
            {...register("pgParams")}
            placeholder="statement_timeout=30000 jit=off search_path='app, public'"
            rows={2}
            spellCheck={false}
            autoComplete="off"
            className="min-h-0 resize-none font-mono text-xs"
          />
        </Row>
      )}
      <Row
        label="Connect timeout"
        htmlFor={`${uid}-timeout`}
        error={formState.errors.connectTimeoutSecs?.message}
        hint={`Seconds; empty means ${DEFAULT_CONNECT_TIMEOUT_SECS}`}
      >
        <Input
          id={`${uid}-timeout`}
          {...register("connectTimeoutSecs")}
          className="w-24 font-mono"
          inputMode="numeric"
          aria-invalid={formState.errors.connectTimeoutSecs ? true : undefined}
        />
      </Row>
    </>
  );
}

/** Fields that live in the (collapsible) Advanced block. */
const ADVANCED_FIELDS = [
  "connectTimeoutSecs",
  "timezone",
  "safeMode",
  "encoding",
  "pgParams",
] as const;

export function ConnectionDialog({
  open,
  profile,
  prefill = null,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  /** Existing profile to edit, or null to create. */
  profile: ConnectionProfile | null;
  /**
   * A new connection's starting values (a connection URL, or the group of "New ▸ Connection in
   * this group…"); skips the driver picker when it sets the driver.
   */
  prefill?: ParsedConnectionUrl | null;
  onOpenChange: (open: boolean) => void;
  onSaved?: (profile: ConnectionProfile) => void;
}) {
  const [id, setId] = useState<string>(() => crypto.randomUUID());
  const form = useForm<ConnectionFormValues>({
    resolver: zodResolver(connectionFormSchema),
    defaultValues: DEFAULT_FORM,
    mode: "onSubmit",
  });
  const save = useSaveProfile();
  const test = useTestConnection();
  const secret = useSecretStatus(profile?.id ?? null);
  const openProfile = useOpenProfile();
  const profiles = useProfiles();
  const storedGroups = useStoredGroups();
  const groups = useMemo(
    () => knownGroups(storedGroups.data ?? [], profiles.data ?? []),
    [storedGroups.data, profiles.data],
  );
  const [step, setStep] = useState<"driver" | "form">("form");
  const [pickedDriver, setPickedDriver] = useState<DatabaseKind>("postgres");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [bootstrapOpen, setBootstrapOpen] = useState(false);
  const [tagsOpen, setTagsOpen] = useState(false);
  const [busy, setBusy] = useState<"save" | "connect" | null>(null);
  const [testing, setTesting] = useState(false);
  /** Saved by Test (or Save) during this dialog session: later saves update it. */
  const savedOnce = useRef(false);
  /** The "Ask every time" password typed for Test, reused by Connect. */
  const askedPassword = useRef<string | null>(null);
  /** The Safe Mode password was confirmed for a protected change during this dialog session. */
  const unlocked = useRef(false);
  const sshRef = useRef<HTMLElement>(null);
  const advancedRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!open) return;
    setId(profile?.id ?? crypto.randomUUID());
    const { connectTimeout: timeout, defaultSafeMode } = getPrefs();
    form.reset(
      profile
        ? profileToForm(profile)
        : prefill
          ? urlToForm(prefill, timeout, defaultSafeMode)
          : newProfileForm(timeout, defaultSafeMode),
    );
    test.reset();
    // A prefill without a driver ("New ▸ Connection in this group…") still picks one first.
    setStep(profile || prefill?.values.driver !== undefined ? "form" : "driver");
    setPickedDriver("postgres");
    setAdvancedOpen(false);
    setBootstrapOpen(false);
    setTagsOpen(false);
    setBusy(null);
    setTesting(false);
    savedOnce.current = false;
    askedPassword.current = null;
    unlocked.current = false;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, profile?.id, prefill]);

  const driver = form.watch("driver");
  const sshEnabled = form.watch("sshEnabled");
  const sqlite = driver === "sqlite";
  const testResult = test.isSuccess ? test.data : undefined;
  const testError = test.isError ? test.error : null;
  const mysqlVersion = form.watch("mysqlVersion");
  const title = profile
    ? `Edit ${profile.name}`
    : `New ${KIND_LABEL[databaseKind(driver, mysqlVersion)]} connection`;

  const buildInput = (values: ConnectionFormValues) => formToProfileInput(values, profile, id);

  const pickDriver = (kind: DatabaseKind) => {
    const d = kindDriver(kind);
    form.setValue("driver", d);
    form.setValue("port", DEFAULT_PORT[d]);
    // MariaDB is the MySQL driver with Ver. ▸ MariaDB (and its SSL mode names).
    if (d === "mysql") {
      const version = kind === "mariadb" ? "mariadb" : "default";
      form.setValue("mysqlVersion", version);
      form.setValue("ssl", sslForVersion(form.getValues("ssl"), version));
    }
    setStep("form");
  };

  /** Reveals the hidden block holding the first invalid field. */
  const onInvalid = (errors: FieldErrors<ConnectionFormValues>) => {
    if (ADVANCED_FIELDS.some((f) => errors[f])) setAdvancedOpen(true);
  };

  /** Saves the form (no toast); null when it failed (useSaveProfile told the user). */
  const persist = async (values: ConnectionFormValues): Promise<ConnectionProfile | null> => {
    const input = buildInput(values);
    // A saved Safe Mode 1 / 2 connection asks for its password before a save that weakens its
    // protection (a lower level, Read-only off, another password), as the toolbar lock does for
    // the level; once per dialog session.
    const changes = profile && !unlocked.current ? protectedChanges(profile, input) : [];
    if (profile && changes.length > 0) {
      if (!(await confirmProtectedChange(profile, changes.join("\n")))) return null;
      unlocked.current = true;
    }
    try {
      const saved = await save.mutateAsync(input);
      rememberSafeModeLevel(saved.id, saved.options.safeMode);
      savedOnce.current = true;
      onSaved?.(saved);
      return saved;
    } catch {
      return null;
    }
  };

  /** Save, or Save + Connect (the form's default action, like TablePlus). */
  const submit = (connect: boolean) =>
    form.handleSubmit(async (values) => {
      setBusy(connect ? "connect" : "save");
      const updating = profile !== null || savedOnce.current;
      const saved = await persist(values);
      setBusy(null);
      // The form stays open when saving failed.
      if (!saved) return;
      if (!connect) toast.success(updating ? "Connection updated" : "Connection created");
      onOpenChange(false);
      if (!connect) return;
      if (values.passwordMode === "ask" && askedPassword.current !== null)
        handOverPassword(saved.id, askedPassword.current);
      // An open session keeps its old settings: Connect switches to it, so say how to apply them.
      if (profile && openConnectionOf(saved.id))
        toast.message("Connection updated", {
          description: `Reconnect (${formatCombo(shortcutFor("connection.reconnect") ?? "Mod+Shift+R")}) to apply the changes to the open session.`,
        });
      openProfile(saved.id);
    }, onInvalid);

  /**
   * Test: saves the (valid) connection first, as TablePlus does, then tries it. An "Ask every
   * time" connection asks for its password (once per dialog session while it works).
   */
  const onTest = async () => {
    const valid = await form.trigger();
    if (!valid) {
      onInvalid(form.formState.errors);
      return;
    }
    const values = form.getValues();
    setTesting(true);
    try {
      if (!(await persist(values))) return;
      const input = buildInput(values);
      if (values.passwordMode === "ask") {
        askedPassword.current ??= await askConnectionPassword(values.name, "test");
        input.password = askedPassword.current;
      }
      // The form's profile id also keys a pending SSH host key.
      const result = await test.mutateAsync(input);
      if (!result.ok) askedPassword.current = null;
    } catch (err) {
      // A failure is shown in the footer; a cancelled prompt shows nothing.
      if (!isPromptCancelled(err)) askedPassword.current = null;
    } finally {
      setTesting(false);
    }
  };

  /**
   * "Import from URL…" of the New connection sheet: closes it and opens the URL dialog, which
   * opens the form again with the link's values.
   */
  const importFromUrl = () => {
    onOpenChange(false);
    openConnectionUrlDialog();
  };

  const toggleSsh = () => {
    const on = !sshEnabled;
    form.setValue("sshEnabled", on, { shouldDirty: true });
    if (!on) return;
    requestAnimationFrame(() => {
      sshRef.current?.scrollIntoView?.({ block: "nearest" });
      if (form.getValues("sshHost") === "") form.setFocus("sshHost");
    });
  };

  const toggleAdvanced = () => {
    setAdvancedOpen((v) => !v);
    if (!advancedOpen)
      requestAnimationFrame(() => advancedRef.current?.scrollIntoView?.({ block: "nearest" }));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* Header and footer stay put; the body scrolls when the window is short (min 600px). */}
      <DialogContent
        className={cn(
          "flex max-h-[calc(100dvh-2rem)] flex-col gap-0 overflow-hidden p-0",
          step === "driver" ? "sm:max-w-lg" : "sm:max-w-[600px]",
        )}
        showCloseButton
      >
        {step === "driver" ? (
          <>
            <DialogHeader className="border-border shrink-0 border-b py-4 pr-12 pl-5">
              <DialogTitle>New connection</DialogTitle>
              <DialogDescription>Choose the database you are connecting to.</DialogDescription>
            </DialogHeader>
            <div className="px-5 py-5">
              <DriverPicker
                value={pickedDriver}
                onChange={setPickedDriver}
                onConfirm={pickDriver}
              />
            </div>
            <DialogFooter className="border-border mx-0 mb-0 shrink-0 border-t px-5 py-3 sm:justify-between">
              <Button type="button" variant="ghost" size="sm" onClick={importFromUrl}>
                <Link2 /> Import from URL…
              </Button>
              <div className="flex items-center gap-2">
                <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
                  Cancel
                </Button>
                <Button type="button" size="sm" onClick={() => pickDriver(pickedDriver)}>
                  Create
                </Button>
              </div>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={submit(true)} className="flex min-h-0 flex-1 flex-col">
            <DialogHeader className="border-border shrink-0 flex-row items-center gap-3 border-b py-3.5 pr-12 pl-5">
              <DriverBadge
                driver={driver}
                mysqlVersion={mysqlVersion}
                color={form.watch("color")}
              />
              <div className="flex min-w-0 flex-col gap-1">
                <DialogTitle className="truncate">{title}</DialogTitle>
                <DialogDescription className="text-xs">
                  Credentials are stored in the {SECRET_STORE}, never in plain files.
                </DialogDescription>
              </div>
              {profile === null && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="ml-auto shrink-0"
                  onClick={importFromUrl}
                  title="Fill the form from a connection URL (postgres://, mysql://, sqlite://…)"
                >
                  <Link2 /> Import from URL…
                </Button>
              )}
            </DialogHeader>
            <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 py-4">
              <GeneralFields
                form={form}
                hasStoredPassword={secret.data?.hasPassword ?? false}
                groups={groups}
                onBootstrap={() => setBootstrapOpen(true)}
                onEditTags={() => setTagsOpen(true)}
              />
              {sshEnabled && (
                <Section title={sqlite ? "SSH (SFTP)" : "SSH tunnel"} sectionRef={sshRef}>
                  <SshFields form={form} />
                </Section>
              )}
              {advancedOpen && (
                <Section title="Advanced" sectionRef={advancedRef}>
                  <AdvancedFields form={form} />
                </Section>
              )}
            </div>
            <DialogFooter className="border-border mx-0 mb-0 shrink-0 flex-row items-center gap-2 border-t px-5 py-3 sm:justify-between">
              <div className="flex shrink-0 items-center gap-1.5">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-pressed={sshEnabled}
                  onClick={toggleSsh}
                  title={
                    sqlite
                      ? "Open a database file on an SSH server (downloaded over SFTP)"
                      : "Connect through an SSH tunnel"
                  }
                  className={cn(
                    sshEnabled &&
                      "border-brand/50 bg-brand-soft text-brand hover:bg-brand-soft hover:text-brand dark:border-brand/50 dark:bg-brand-soft",
                  )}
                >
                  <Network /> Over SSH
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-expanded={advancedOpen}
                  onClick={toggleAdvanced}
                >
                  <ChevronRight
                    className={cn("transition-transform", advancedOpen && "rotate-90")}
                  />
                  Advanced
                </Button>
              </div>
              <div className="flex min-w-0 flex-1 justify-center">
                <TestStatus result={testResult} error={testError} />
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={onTest}
                  disabled={test.isPending || testing || busy !== null}
                  title="Save and test the connection"
                >
                  {test.isPending || testing ? <Loader2 className="animate-spin" /> : null}
                  Test
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={submit(false)}
                  disabled={busy !== null}
                >
                  {busy === "save" ? <Loader2 className="animate-spin" /> : null}
                  Save
                </Button>
                <Button type="submit" size="sm" disabled={busy !== null}>
                  {busy === "connect" ? <Loader2 className="animate-spin" /> : null}
                  Connect
                </Button>
              </div>
            </DialogFooter>
          </form>
        )}
        {/* Nested inside the form's content, so clicks in it do not dismiss the form. */}
        <BootstrapCommandsDialog
          open={bootstrapOpen}
          value={form.watch("startupSql")}
          script={form.watch("bootstrapScript")}
          driver={driver}
          onOpenChange={setBootstrapOpen}
          onSave={(sql, script) => {
            form.setValue("startupSql", sql, { shouldDirty: true });
            form.setValue("bootstrapScript", script, { shouldDirty: true });
          }}
        />
        <EditTagsDialog
          open={tagsOpen}
          onOpenChange={setTagsOpen}
          current={form.watch("tag")}
          onRenamed={(from, to) => {
            if (form.getValues("tag") === from) form.setValue("tag", to, { shouldDirty: true });
          }}
        />
      </DialogContent>
    </Dialog>
  );
}

/** The last Test's outcome: server version, latency and tunnel, or why it failed. */
function TestStatus({
  result,
  error,
}: {
  result: ReturnType<typeof useTestConnection>["data"];
  error: Error | null;
}) {
  if (result)
    return (
      <span
        role="status"
        className={cn(
          "flex min-w-0 items-center gap-1 text-xs",
          result.ok ? "text-success" : "text-error",
        )}
        title={result.message}
      >
        {result.ok ? <Check className="size-3.5 shrink-0" /> : <X className="size-3.5 shrink-0" />}
        <span className="truncate">
          {result.ok
            ? `${result.serverVersion ?? "OK"} · ${result.latencyMs} ms${result.tunnelUsed ? " · via SSH" : ""}`
            : result.message}
        </span>
      </span>
    );
  if (error) {
    // The SSH 2FA question was cancelled: nothing went wrong.
    const declined = hostKeyDeclined(error) || toIpcError(error).code === "CANCELLED";
    return (
      <span
        role="status"
        className={cn(
          "flex min-w-0 items-center gap-1 text-xs",
          declined ? "text-muted-foreground" : "text-error",
        )}
        title={error.message}
      >
        {declined ? (
          <ShieldOff className="size-3.5 shrink-0" />
        ) : (
          <X className="size-3.5 shrink-0" />
        )}
        <span className="truncate">
          {!declined
            ? error.message
            : hostKeyDeclined(error)
              ? "Host key not trusted"
              : "Cancelled"}
        </span>
      </span>
    );
  }
  return null;
}
