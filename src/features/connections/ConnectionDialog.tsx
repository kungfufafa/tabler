import { zodResolver } from "@hookform/resolvers/zod";
import {
  Check,
  ChevronRight,
  FilePlus2,
  FolderOpen,
  Info,
  Loader2,
  Network,
  ScrollText,
  ShieldOff,
  X,
} from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Controller, useForm, type FieldErrors, type UseFormReturn } from "react-hook-form";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { rememberSafeModeLevel } from "@/features/safety/guard";
import { confirmProtectedChange, protectedChanges } from "@/features/safety/setLevel";
import {
  ipc,
  isMock,
  toIpcError,
  type ConnectionProfile,
  type ConnectionTag,
  type Driver,
  type PasswordMode,
  type SafeModeLevel,
  type SslMode,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { ACCENT_PRESETS } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { getPrefs } from "@/stores/prefs";
import { DriverBadge } from "./badges";
import { BootstrapCommandsDialog } from "./BootstrapCommandsDialog";
import { accentColor, DRIVER_LABEL, DRIVER_TINT, DRIVERS, TAG_COLOR, TAG_LABEL } from "./colors";
import { knownGroups, useStoredGroups } from "./groups";
import { openConnectionOf, useOpenProfile } from "./openProfile";
import { askConnectionPassword, handOverPassword, isPromptCancelled } from "./passwordPromptStore";
import {
  CONNECTION_TAGS,
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  newProfileForm,
  PASSWORD_MODE_LABEL,
  PASSWORD_MODES,
  profileToForm,
  SAFE_MODE_LABEL,
  SAFE_MODES,
  urlToForm,
  type ConnectionFormValues,
} from "./schema";
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

/** SSL modes per driver, in menu order: libpq's names for PostgreSQL, MySQL's for MySQL. */
const SSL_MODES: Record<Exclude<Driver, "sqlite">, ReadonlyArray<[SslMode, string]>> = {
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
};

const SAFE_MODE_HINT: Record<SafeModeLevel, string> = {
  silent: "Send queries to the server without any warnings",
  alert: "Warn before sending queries to the server",
  alertNonSelect: "Warn before sending queries, except SELECT, EXPLAIN and SHOW",
  safe: "Ask for the password before sending queries to the server",
  safeNonSelect: "Ask for the password before sending queries, except SELECT, EXPLAIN and SHOW",
};

const PASSWORD_HINT: Record<Exclude<PasswordMode, "keychain">, string> = {
  ask: "Asked on every connect; kept in memory while connected",
  none: "Connects without a password",
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
  try {
    await ipc.profiles.createSqliteFile(path);
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
 * New connection, step 1: three big driver tiles. A click selects, a double-click or ↩ (or the
 * dialog's Create button) goes on to the form; ←/→ move between the tiles.
 */
function DriverPicker({
  value,
  onChange,
  onConfirm,
}: {
  value: Driver;
  onChange: (driver: Driver) => void;
  onConfirm: (driver: Driver) => void;
}) {
  const tiles = useRef(new Map<Driver, HTMLButtonElement>());
  return (
    <div role="radiogroup" aria-label="Database driver" className="grid grid-cols-3 gap-3">
      {DRIVERS.map((d, i) => (
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
            const next = DRIVERS[(i + step + DRIVERS.length) % DRIVERS.length]!;
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
          <DriverBadge driver={d} color={DRIVER_TINT[d]} size="lg" />
          <span className="text-[13px] font-medium">{DRIVER_LABEL[d]}</span>
        </button>
      ))}
    </div>
  );
}

/** "Bootstrap commands…" and a one-line preview of what is set. */
function BootstrapRow({ form, onOpen }: { form: Form; onOpen: () => void }) {
  const sql = form.watch("startupSql").trim();
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
      </div>
    </Row>
  );
}

function GeneralFields({
  form,
  hasStoredPassword,
  groups,
  onBootstrap,
}: {
  form: Form;
  hasStoredPassword: boolean;
  /** Existing group names, suggested in the Group field. */
  groups: readonly string[];
  onBootstrap: () => void;
}) {
  const { register, control, formState, watch, setValue, getValues } = form;
  const errors = formState.errors;
  const uid = useId();
  const driver = watch("driver");
  const clearPassword = watch("clearPassword");
  const passwordMode = watch("passwordMode");
  const ssl = watch("ssl");
  const sqlite = driver === "sqlite";
  const keychain = passwordMode === "keychain";
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
                onValueChange={(v) => field.onChange(v as ConnectionTag)}
              >
                <SelectTrigger id={`${uid}-tag`} className="w-36">
                  <SelectValue placeholder="None" />
                </SelectTrigger>
                <SelectContent>
                  {CONNECTION_TAGS.map((t) => (
                    <SelectItem key={t} value={t}>
                      <span
                        aria-hidden
                        className="size-2 rounded-full"
                        style={{ background: accentColor(TAG_COLOR[t]) }}
                      />
                      {TAG_LABEL[t]}
                    </SelectItem>
                  ))}
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
              </div>
            )}
          />
        </div>
      </Row>
      <Row label="Group" htmlFor={`${uid}-group`}>
        <Input
          id={`${uid}-group`}
          {...register("group")}
          placeholder="Optional, groups connections in the list"
          list={`${uid}-groups`}
          autoComplete="off"
        />
        <datalist id={`${uid}-groups`}>
          {groups.map((g) => (
            <option key={g} value={g} />
          ))}
        </datalist>
      </Row>
      <Row label="Driver" htmlFor={`${uid}-driver`}>
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
      </Row>

      {sqlite ? (
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
              title="Create a new, empty database file"
              onClick={() => void createSqliteFile(form)}
            >
              <FilePlus2 /> New…
            </Button>
          </div>
        </Row>
      ) : null}
      {sqlite ? (
        <BootstrapRow form={form} onOpen={onBootstrap} />
      ) : (
        <>
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
            </div>
          </Row>
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
                    : "Stored in the macOS Keychain"
            }
          >
            <div className="flex items-center gap-2">
              <Input
                id={`${uid}-password`}
                {...register("password")}
                type="password"
                placeholder={
                  passwordMode === "ask"
                    ? "Asked when connecting"
                    : passwordMode === "none"
                      ? "No password"
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
                      {PASSWORD_MODES.map((m) => (
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
          <Row
            label="Database"
            htmlFor={`${uid}-database`}
            hint={driver === "postgres" ? "Defaults to postgres" : "Optional"}
          >
            <Input id={`${uid}-database`} {...register("database")} className="font-mono" />
          </Row>
          <BootstrapRow form={form} onOpen={onBootstrap} />
          <Row label="SSL mode" htmlFor={`${uid}-ssl`}>
            <Controller
              control={control}
              name="ssl"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id={`${uid}-ssl`} className="w-48">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {SSL_MODES[driver === "mysql" ? "mysql" : "postgres"].map(([m, label]) => (
                      <SelectItem key={m} value={m}>
                        {label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </Row>
          {sslKeys && (
            <Row
              label="SSL keys"
              error={errors.sslKeyPath?.message ?? errors.sslCertPath?.message}
              hint="Client key and certificate (PEM) for servers that check them; the CA certificate for verify-ca / verify-full"
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
        </>
      )}
    </div>
  );
}

function SshFields({ form }: { form: Form }) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const uid = useId();
  const auth = watch("sshAuth");
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
        <Row label="SSH password" htmlFor={`${uid}-password`}>
          <Input
            id={`${uid}-password`}
            {...register("sshPassword")}
            type="password"
            className="font-mono"
            autoComplete="new-password"
          />
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
        label="Local port"
        htmlFor={`${uid}-local-port`}
        error={errors.sshLocalPort?.message ?? errors.sshKeepaliveSecs?.message}
      >
        <div className="flex items-center gap-2">
          <Input
            id={`${uid}-local-port`}
            {...register("sshLocalPort")}
            placeholder="auto"
            className="w-24 shrink-0 font-mono"
            inputMode="numeric"
            aria-invalid={errors.sshLocalPort ? true : undefined}
          />
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
      <Row label="Safe mode" htmlFor={`${uid}-safe`} hint={SAFE_MODE_HINT[safeMode]}>
        <Controller
          control={control}
          name="safeMode"
          render={({ field }) => (
            <Select value={field.value} onValueChange={(v) => field.onChange(v as SafeModeLevel)}>
              <SelectTrigger id={`${uid}-safe`} className="w-48">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {SAFE_MODES.map((m) => (
                  <SelectItem key={m} value={m}>
                    {SAFE_MODE_LABEL[m]}
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
      <Row
        label="Connect timeout"
        htmlFor={`${uid}-timeout`}
        error={formState.errors.connectTimeoutSecs?.message}
        hint="Seconds; empty means 15"
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
const ADVANCED_FIELDS = ["connectTimeoutSecs", "timezone", "safeMode"] as const;

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
  /** A new connection's starting values (from a connection URL); skips the driver picker. */
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
  const [pickedDriver, setPickedDriver] = useState<Driver>("postgres");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [bootstrapOpen, setBootstrapOpen] = useState(false);
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
    setStep(profile || prefill ? "form" : "driver");
    setPickedDriver("postgres");
    setAdvancedOpen(false);
    setBootstrapOpen(false);
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
  const title = profile ? `Edit ${profile.name}` : `New ${DRIVER_LABEL[driver]} connection`;

  const buildInput = (values: ConnectionFormValues) => formToProfileInput(values, profile, id);

  const pickDriver = (d: Driver) => {
    form.setValue("driver", d);
    form.setValue("port", DEFAULT_PORT[d]);
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
          step === "driver" ? "sm:max-w-md" : "sm:max-w-[600px]",
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
            <DialogFooter className="border-border mx-0 mb-0 shrink-0 border-t px-5 py-3">
              <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="button" size="sm" onClick={() => pickDriver(pickedDriver)}>
                Create
              </Button>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={submit(true)} className="flex min-h-0 flex-1 flex-col">
            <DialogHeader className="border-border shrink-0 flex-row items-center gap-3 border-b py-3.5 pr-12 pl-5">
              <DriverBadge driver={driver} color={form.watch("color")} />
              <div className="flex min-w-0 flex-col gap-1">
                <DialogTitle className="truncate">{title}</DialogTitle>
                <DialogDescription className="text-xs">
                  Credentials are stored in the macOS Keychain, never in plain files.
                </DialogDescription>
              </div>
            </DialogHeader>
            <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 py-4">
              <GeneralFields
                form={form}
                hasStoredPassword={secret.data?.hasPassword ?? false}
                groups={groups}
                onBootstrap={() => setBootstrapOpen(true)}
              />
              {sshEnabled && !sqlite && (
                <Section title="SSH tunnel" sectionRef={sshRef}>
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
                {!sqlite && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    aria-pressed={sshEnabled}
                    onClick={toggleSsh}
                    title="Connect through an SSH tunnel"
                    className={cn(
                      sshEnabled &&
                        "border-brand/50 bg-brand-soft text-brand hover:bg-brand-soft hover:text-brand dark:border-brand/50 dark:bg-brand-soft",
                    )}
                  >
                    <Network /> Over SSH
                  </Button>
                )}
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
          driver={driver}
          onOpenChange={setBootstrapOpen}
          onSave={(sql) => form.setValue("startupSql", sql, { shouldDirty: true })}
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
    const declined = hostKeyDeclined(error);
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
        <span className="truncate">{declined ? "Host key not trusted" : error.message}</span>
      </span>
    );
  }
  return null;
}
