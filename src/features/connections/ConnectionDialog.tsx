import { zodResolver } from "@hookform/resolvers/zod";
import { ArrowRight, Check, FolderOpen, Info, Loader2, Network, ShieldOff, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Controller, useForm, type UseFormReturn } from "react-hook-form";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { isMock, type ConnectionProfile, type Driver } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { getPrefs } from "@/stores/prefs";
import { accentColor, DRIVER_LABEL } from "./colors";
import {
  connectionFormSchema,
  DEFAULT_FORM,
  formToProfileInput,
  newProfileForm,
  profileToForm,
  type ConnectionFormValues,
} from "./schema";
import {
  hostKeyDeclined,
  useSaveProfile,
  useSecretStatus,
  useTestConnection,
} from "./useConnections";

type Form = UseFormReturn<ConnectionFormValues>;

function Field({
  label,
  error,
  children,
  className,
  hint,
}: {
  label: string;
  error?: string;
  children: React.ReactNode;
  className?: string;
  hint?: string;
}) {
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <Label className="text-muted-foreground text-xs">{label}</Label>
      {children}
      {error ? (
        <p className="text-error text-[11px]">{error}</p>
      ) : hint ? (
        <p className="text-muted-foreground text-[11px]">{hint}</p>
      ) : null}
    </div>
  );
}

async function pickSqliteFile(): Promise<string | null> {
  if (isMock) return window.prompt("Path to SQLite file", "/Users/me/data.sqlite");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    multiple: false,
    directory: false,
    title: "Open SQLite database",
    filters: [{ name: "SQLite", extensions: ["sqlite", "sqlite3", "db", "db3", "s3db"] }],
  });
  return typeof picked === "string" ? picked : null;
}

async function pickFile(title: string): Promise<string | null> {
  if (isMock) return window.prompt(title, "");
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, directory: false, title });
  return typeof picked === "string" ? picked : null;
}

/** "via ssh deploy@bastion:22" under the host fields while the SSH tunnel is on. */
function SshSummary({ form, onEdit }: { form: Form; onEdit: () => void }) {
  const { watch } = form;
  const host = watch("sshHost").trim();
  const user = watch("sshUser").trim();
  const port = watch("sshPort").trim() || "22";
  const configured = host !== "" && user !== "";
  return (
    <button
      type="button"
      onClick={onEdit}
      className="border-border bg-surface-2 hover:border-border-strong group col-span-6 flex h-7 min-w-0 items-center gap-1.5 rounded-md border px-2 text-left text-[11px]"
      title="Edit SSH tunnel"
    >
      <Network className="text-brand size-3.5 shrink-0" />
      {configured ? (
        <span className="text-muted-foreground min-w-0 truncate">
          via ssh{" "}
          <span className="text-foreground font-mono">
            {user}@{host}:{port}
          </span>
        </span>
      ) : (
        <span className="text-warning truncate">SSH tunnel is on but not configured</span>
      )}
      <ArrowRight className="text-muted-foreground ml-auto size-3 shrink-0 opacity-0 transition-opacity group-hover:opacity-100" />
    </button>
  );
}

function GeneralTab({
  form,
  hasStoredPassword,
  onEditSsh,
}: {
  form: Form;
  hasStoredPassword: boolean;
  onEditSsh: () => void;
}) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const driver = watch("driver");
  const clearPassword = watch("clearPassword");
  const sshEnabled = watch("sshEnabled");
  const sqlite = driver === "sqlite";
  return (
    <div className="grid grid-cols-6 gap-3">
      <Field label="Name" error={errors.name?.message} className="col-span-4">
        <Input {...register("name")} placeholder="My database" autoFocus />
      </Field>
      <Field label="Color" className="col-span-2">
        <Controller
          control={control}
          name="color"
          render={({ field }) => (
            <div className="flex h-8 items-center gap-1.5">
              {ACCENT_PRESETS.map((c) => (
                <button
                  key={c}
                  type="button"
                  aria-label={c}
                  onClick={() => field.onChange(c)}
                  className={cn(
                    "ring-offset-background size-4 rounded-full ring-offset-2 transition-transform",
                    field.value === c ? "ring-foreground/60 scale-110 ring-2" : "hover:scale-110",
                  )}
                  style={{ background: accentColor(c) }}
                />
              ))}
            </div>
          )}
        />
      </Field>
      <Field label="Driver" className="col-span-3">
        <Controller
          control={control}
          name="driver"
          render={({ field }) => (
            <Select
              value={field.value}
              onValueChange={(v) => {
                field.onChange(v as Driver);
                if (v === "postgres") setValue("port", "5432");
                if (v === "mysql") setValue("port", "3306");
              }}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(Object.keys(DRIVER_LABEL) as Driver[]).map((d) => (
                  <SelectItem key={d} value={d}>
                    {DRIVER_LABEL[d]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      </Field>
      <Field label="Group" className="col-span-3" hint="Optional, groups connections in the list">
        <Input {...register("group")} placeholder="e.g. Local, Staging" />
      </Field>

      {sqlite ? (
        <Field label="Database file" error={errors.sqlitePath?.message} className="col-span-6">
          <div className="flex gap-2">
            <Input
              {...register("sqlitePath")}
              placeholder="/path/to/database.sqlite"
              className="font-mono"
            />
            <Button
              type="button"
              variant="outline"
              onClick={async () => {
                const p = await pickSqliteFile();
                if (p) setValue("sqlitePath", p, { shouldValidate: true });
              }}
            >
              <FolderOpen /> Browse
            </Button>
          </div>
        </Field>
      ) : (
        <>
          <Field label="Host" error={errors.host?.message} className="col-span-4">
            <Input {...register("host")} placeholder="127.0.0.1" className="font-mono" />
          </Field>
          <Field label="Port" error={errors.port?.message} className="col-span-2">
            <Input
              {...register("port")}
              placeholder={driver === "postgres" ? "5432" : "3306"}
              className="font-mono"
              inputMode="numeric"
            />
          </Field>
          {sshEnabled && <SshSummary form={form} onEdit={onEditSsh} />}
          <Field label="User" error={errors.user?.message} className="col-span-3">
            <Input
              {...register("user")}
              placeholder={driver === "postgres" ? "postgres" : "root"}
              className="font-mono"
              autoComplete="off"
            />
          </Field>
          <Field
            label="Password"
            className="col-span-3"
            hint={
              hasStoredPassword && !clearPassword
                ? "Stored in Keychain — leave empty to keep"
                : undefined
            }
          >
            <div className="flex items-center gap-2">
              <Input
                {...register("password")}
                type="password"
                placeholder={hasStoredPassword && !clearPassword ? "••••••••" : ""}
                className="font-mono"
                autoComplete="new-password"
                disabled={clearPassword}
              />
              {hasStoredPassword && (
                <label className="text-muted-foreground flex items-center gap-1 text-[11px] whitespace-nowrap">
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
                  clear
                </label>
              )}
            </div>
          </Field>
          <Field
            label="Database"
            className="col-span-3"
            hint={driver === "postgres" ? "Defaults to postgres" : "Optional"}
          >
            <Input {...register("database")} className="font-mono" />
          </Field>
          <Field label="SSL" className="col-span-3">
            <Controller
              control={control}
              name="ssl"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="disable">Disable</SelectItem>
                    <SelectItem value="prefer">Prefer (default)</SelectItem>
                    <SelectItem value="require">Require</SelectItem>
                    <SelectItem value="verify-ca">Verify CA</SelectItem>
                    <SelectItem value="verify-full">Verify full</SelectItem>
                  </SelectContent>
                </Select>
              )}
            />
          </Field>
        </>
      )}
    </div>
  );
}

function SshTab({ form }: { form: Form }) {
  const { register, control, formState, watch, setValue } = form;
  const errors = formState.errors;
  const enabled = watch("sshEnabled");
  const auth = watch("sshAuth");
  return (
    <div className="flex flex-col gap-3">
      <label className="border-border bg-surface-2 flex items-center justify-between rounded-md border px-3 py-2">
        <div>
          <div className="text-sm font-medium">Connect over SSH tunnel</div>
          <div className="text-muted-foreground text-xs">
            Forward the database port through an SSH server.
          </div>
        </div>
        <Controller
          control={control}
          name="sshEnabled"
          render={({ field }) => <Switch checked={field.value} onCheckedChange={field.onChange} />}
        />
      </label>
      <div className={cn("grid grid-cols-6 gap-3", !enabled && "pointer-events-none opacity-50")}>
        <Field label="SSH host" error={errors.sshHost?.message} className="col-span-4">
          <Input {...register("sshHost")} placeholder="bastion.example.com" className="font-mono" />
        </Field>
        <Field label="Port" error={errors.sshPort?.message} className="col-span-2">
          <Input {...register("sshPort")} className="font-mono" inputMode="numeric" />
        </Field>
        <Field label="SSH user" error={errors.sshUser?.message} className="col-span-3">
          <Input {...register("sshUser")} className="font-mono" autoComplete="off" />
        </Field>
        <Field label="Authentication" className="col-span-3">
          <Controller
            control={control}
            name="sshAuth"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger className="w-full">
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
        </Field>
        {auth === "key" && (
          <>
            <Field label="Private key" error={errors.sshKeyPath?.message} className="col-span-6">
              <div className="flex gap-2">
                <Input
                  {...register("sshKeyPath")}
                  placeholder="~/.ssh/id_ed25519"
                  className="font-mono"
                />
                <Button
                  type="button"
                  variant="outline"
                  onClick={async () => {
                    const p = await pickFile("Choose private key");
                    if (p) setValue("sshKeyPath", p, { shouldValidate: true });
                  }}
                >
                  <FolderOpen /> Browse
                </Button>
              </div>
            </Field>
            <Field
              label="Key passphrase"
              className="col-span-6"
              hint="Leave empty if the key is not encrypted"
            >
              <Input
                {...register("sshPassphrase")}
                type="password"
                className="font-mono"
                autoComplete="new-password"
              />
            </Field>
          </>
        )}
        {auth === "password" && (
          <Field label="SSH password" className="col-span-6">
            <Input
              {...register("sshPassword")}
              type="password"
              className="font-mono"
              autoComplete="new-password"
            />
          </Field>
        )}
        {auth === "agent" && (
          <p className="text-muted-foreground col-span-6 text-[11px]">
            Uses the identities loaded in your SSH agent (<span className="font-mono">ssh-add</span>
            ).
          </p>
        )}
        <div className="border-border text-muted-foreground col-span-6 flex items-center gap-1.5 border-t pt-2.5 text-[11px]">
          <Info className="size-3.5 shrink-0" />
          <span className="min-w-0">
            Local port: <span className="text-foreground font-mono">auto</span> · keepalive is
            automatic · the host key is confirmed on first connect
          </span>
        </div>
      </div>
    </div>
  );
}

function AdvancedTab({ form }: { form: Form }) {
  const { register, control, formState, watch } = form;
  const driver = watch("driver");
  return (
    <div className="grid grid-cols-6 gap-3">
      <label className="border-border bg-surface-2 col-span-6 flex items-center justify-between rounded-md border px-3 py-2">
        <div>
          <div className="text-sm font-medium">Read-only session</div>
          <div className="text-muted-foreground text-xs">
            Open the connection in read-only mode (no edits, no DDL).
          </div>
        </div>
        <Controller
          control={control}
          name="readOnly"
          render={({ field }) => <Switch checked={field.value} onCheckedChange={field.onChange} />}
        />
      </label>
      {driver !== "sqlite" && (
        <Field
          label="Session time zone"
          className="col-span-3"
          hint="e.g. UTC or Asia/Jakarta; empty keeps the server default"
        >
          <Input {...register("timezone")} className="font-mono" />
        </Field>
      )}
      <Field
        label="Connect timeout (s)"
        error={formState.errors.connectTimeoutSecs?.message}
        className="col-span-3"
        hint="Empty means 15"
      >
        <Input {...register("connectTimeoutSecs")} className="font-mono" inputMode="numeric" />
      </Field>
      {driver !== "sqlite" && (
        <Field
          label="CA certificate (PEM)"
          className="col-span-6"
          hint="Used with Verify CA / Verify full"
        >
          <Input {...register("sslCaPath")} className="font-mono" placeholder="/path/to/ca.pem" />
        </Field>
      )}
    </div>
  );
}

export function ConnectionDialog({
  open,
  profile,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  /** Existing profile to edit, or null to create. */
  profile: ConnectionProfile | null;
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
  const [tab, setTab] = useState("general");

  useEffect(() => {
    if (!open) return;
    setId(profile?.id ?? crypto.randomUUID());
    form.reset(profile ? profileToForm(profile) : newProfileForm(getPrefs().connectTimeout));
    test.reset();
    setTab("general");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, profile?.id]);

  const testResult = test.isSuccess ? test.data : undefined;
  const testError = test.isError ? test.error : null;
  const title = useMemo(() => (profile ? `Edit ${profile.name}` : "New connection"), [profile]);

  const buildInput = (values: ConnectionFormValues) => formToProfileInput(values, profile, id);

  const onSubmit = form.handleSubmit(async (values) => {
    const saved = await save.mutateAsync(buildInput(values));
    toast.success(profile ? "Connection updated" : "Connection created");
    onSaved?.(saved);
    onOpenChange(false);
  });

  const onTest = async () => {
    const valid = await form.trigger();
    if (!valid) return;
    try {
      // Uses the form's (possibly unsaved) profile id, which also keys a pending SSH host key.
      await test.mutateAsync(buildInput(form.getValues()));
    } catch {
      // Shown next to the Test button.
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* Header and footer stay put; the tab body scrolls when the window is short (min 600px). */}
      <DialogContent
        className="flex max-h-[calc(100dvh-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-xl"
        showCloseButton
      >
        <form onSubmit={onSubmit} className="flex min-h-0 flex-1 flex-col">
          <DialogHeader className="border-border shrink-0 border-b px-5 py-4">
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>
              Credentials are stored in the macOS Keychain, never in plain files.
            </DialogDescription>
          </DialogHeader>
          <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1 pt-3">
            <TabsList className="mx-5 shrink-0">
              <TabsTrigger value="general">General</TabsTrigger>
              <TabsTrigger value="ssh" disabled={form.watch("driver") === "sqlite"}>
                SSH
              </TabsTrigger>
              <TabsTrigger value="advanced">Advanced</TabsTrigger>
            </TabsList>
            <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
              <TabsContent value="general" className="min-h-[268px]">
                <GeneralTab
                  form={form}
                  hasStoredPassword={secret.data?.hasPassword ?? false}
                  onEditSsh={() => setTab("ssh")}
                />
              </TabsContent>
              <TabsContent value="ssh" className="min-h-[268px]">
                <SshTab form={form} />
              </TabsContent>
              <TabsContent value="advanced" className="min-h-[268px]">
                <AdvancedTab form={form} />
              </TabsContent>
            </div>
          </Tabs>
          <DialogFooter className="border-border mx-0 mb-0 shrink-0 items-center border-t px-5 py-3 sm:justify-between">
            <div className="flex min-w-0 items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={onTest}
                disabled={test.isPending}
              >
                {test.isPending ? <Loader2 className="animate-spin" /> : null}
                Test
              </Button>
              {testResult && (
                <span
                  className={cn(
                    "flex min-w-0 items-center gap-1 truncate text-xs",
                    testResult.ok ? "text-success" : "text-error",
                  )}
                  title={testResult.message}
                >
                  {testResult.ok ? (
                    <Check className="size-3.5 shrink-0" />
                  ) : (
                    <X className="size-3.5 shrink-0" />
                  )}
                  <span className="truncate">
                    {testResult.ok
                      ? `${testResult.serverVersion ?? "OK"} · ${testResult.latencyMs} ms${testResult.tunnelUsed ? " · via SSH" : ""}`
                      : testResult.message}
                  </span>
                </span>
              )}
              {testError && (
                <span
                  role="status"
                  className={cn(
                    "flex min-w-0 items-center gap-1 truncate text-xs",
                    hostKeyDeclined(testError) ? "text-muted-foreground" : "text-error",
                  )}
                  title={testError.message}
                >
                  {hostKeyDeclined(testError) ? (
                    <ShieldOff className="size-3.5 shrink-0" />
                  ) : (
                    <X className="size-3.5 shrink-0" />
                  )}
                  <span className="truncate">
                    {hostKeyDeclined(testError) ? "Host key not trusted" : testError.message}
                  </span>
                </span>
              )}
            </div>
            <div className="flex items-center gap-2">
              <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" size="sm" disabled={save.isPending}>
                {save.isPending ? <Loader2 className="animate-spin" /> : null}
                {profile ? "Save" : "Create"}
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
