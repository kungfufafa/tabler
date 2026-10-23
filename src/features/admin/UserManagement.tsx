import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronRight,
  Eye,
  EyeOff,
  Loader2,
  Minus,
  Plus,
  ShieldCheck,
  TriangleAlert,
  UserRound,
  Users,
} from "lucide-react";
import { useEffect, useId, useMemo, useState, type KeyboardEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useDatabases } from "@/features/sidebar/useSchemaTree";
import { SqlBlock } from "@/features/structure/SqlBlock";
import {
  ipc,
  type Driver,
  type ResourceLimits,
  type RoleAttributes,
  type UserState,
  type UserSummary,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { adminKeys } from "./keys";
import { ErrorLine, Field, NativeSelect } from "./parts";
import { errorInfo, guarded, type AdminErrorInfo } from "./run";
import {
  draftOf,
  draftProblems,
  dropChange,
  isDirty,
  MYSQL_DB_PRIVILEGES,
  MYSQL_GLOBAL_PRIVILEGES,
  newDraft,
  PG_DATABASE_PRIVILEGES,
  PG_SCHEMA_PRIVILEGES,
  PG_TABLE_PRIVILEGES,
  privilegeDatabases,
  privilegesOn,
  setPrivilege,
  setPrivileges,
  setTablePrivilege,
  tablePrivilegeState,
  toChange,
  userKey,
  userLabel,
  userRef,
  type UserDraft,
} from "./users";

/** Whether `u` is the connection's account (MySQL reports `CURRENT_USER()` as `user@host`). */
function isAccount(u: { name: string; host: string | null }, currentUser: string | null): boolean {
  if (currentUser === null) return false;
  return u.host === null ? u.name === currentUser : `${u.name}@${u.host}` === currentUser;
}

/** Key of the account being created. */
const NEW_KEY = "\u0000new";

/** What waits for the "Discard changes?" answer. */
type Pending = { kind: "select"; key: string } | { kind: "new" } | { kind: "close" };

/** Connection ▸ User Management…: accounts on the left, the selected one's editor on the right. */
export function UserManagementDialog({
  connectionId,
  driver,
  connectionName,
  currentDatabase,
  currentUser,
  onClose,
}: {
  connectionId: string;
  driver: Driver;
  connectionName: string;
  currentDatabase: string | null;
  currentUser: string | null;
  onClose: () => void;
}) {
  const [requestClose, setRequestClose] = useState(0);
  return (
    <Dialog open onOpenChange={(open) => !open && setRequestClose((n) => n + 1)}>
      <DialogContent className="flex h-[min(720px,85vh)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[min(980px,calc(100%-2rem))]">
        <header className="border-border flex h-11 shrink-0 items-center gap-2 border-b pr-12 pl-4">
          <DialogTitle className="text-[13px] font-semibold">User Management</DialogTitle>
          <DialogDescription className="text-muted-foreground truncate text-xs">
            {connectionName}
          </DialogDescription>
        </header>
        {driver === "sqlite" ? (
          <EmptyCloser requestClose={requestClose} onClose={onClose}>
            <EmptyState
              icon={Users}
              title="User management is not available"
              description="SQLite databases have no users or privileges."
            />
          </EmptyCloser>
        ) : (
          <UserManagementBody
            connectionId={connectionId}
            driver={driver}
            currentDatabase={currentDatabase}
            currentUser={currentUser}
            requestClose={requestClose}
            onClose={onClose}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Closes right away when asked (nothing to lose). */
function EmptyCloser({
  requestClose,
  onClose,
  children,
}: {
  requestClose: number;
  onClose: () => void;
  children: ReactNode;
}) {
  useEffect(() => {
    if (requestClose > 0) onClose();
  }, [requestClose, onClose]);
  return children;
}

function UserManagementBody({
  connectionId,
  driver,
  currentDatabase,
  currentUser,
  requestClose,
  onClose,
}: {
  connectionId: string;
  driver: Driver;
  currentDatabase: string | null;
  currentUser: string | null;
  requestClose: number;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [includeNologin, setIncludeNologin] = useState(false);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [draft, setDraft] = useState<{ key: string; value: UserDraft } | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [confirmDrop, setConfirmDrop] = useState<UserState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ info: AdminErrorInfo; executed: string[] } | null>(null);
  const [showSql, setShowSql] = useState(false);

  const users = useQuery({
    queryKey: adminKeys.userList(connectionId, driver === "postgres" && includeNologin),
    queryFn: () => ipc.admin.users(connectionId, driver === "postgres" && includeNologin),
    placeholderData: keepPreviousData,
    retry: false,
  });
  const list = useMemo(() => users.data ?? [], [users.data]);
  const selectedSummary =
    selectedKey && selectedKey !== NEW_KEY
      ? (list.find((u) => userKey(u) === selectedKey) ?? null)
      : null;
  const detail = useQuery({
    queryKey: adminKeys.userDetail(connectionId, selectedKey ?? ""),
    queryFn: () => ipc.admin.user(connectionId, userRef(selectedSummary!)),
    enabled: selectedSummary !== null,
    retry: false,
  });

  // The loaded account becomes the working copy (once per selection).
  useEffect(() => {
    if (!selectedKey || selectedKey === NEW_KEY || !detail.data) return;
    if (draft?.key === selectedKey) return;
    setDraft({ key: selectedKey, value: draftOf(detail.data.state) });
  }, [selectedKey, detail.data, draft?.key]);

  const current = draft && draft.key === selectedKey ? draft.value : null;
  const dirty = current !== null && isDirty(current);

  const proceed = (next: Pending) => {
    setError(null);
    setShowSql(false);
    if (next.kind === "close") return onClose();
    if (next.kind === "new") {
      setSelectedKey(NEW_KEY);
      setDraft({ key: NEW_KEY, value: newDraft(driver) });
      return;
    }
    setSelectedKey(next.key);
    setDraft(null);
  };
  const request = (next: Pending) => {
    if (busy) return;
    if (dirty) setPending(next);
    else proceed(next);
  };

  // The dialog's close button / Esc / overlay ask before unsaved edits go.
  useEffect(() => {
    if (requestClose > 0) request({ kind: "close" });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only on a new close request
  }, [requestClose]);

  const update = (state: UserState) =>
    setDraft((d) => (d ? { ...d, value: { ...d.value, state } } : d));
  const setPassword = (password: string | null) =>
    setDraft((d) => (d ? { ...d, value: { ...d.value, password } } : d));

  const refresh = async (key: string | null) => {
    await qc.invalidateQueries({ queryKey: adminKeys.users(connectionId) });
    qc.removeQueries({ queryKey: adminKeys.userDetails(connectionId) });
    setDraft(null);
    setSelectedKey(key);
  };

  const apply = async () => {
    if (!current || busy || !dirty || draftProblems(current, driver).length > 0) return;
    const change = toChange(current);
    const target = change.edited ?? current.state;
    setBusy(true);
    setError(null);
    const outcome = await guarded(
      connectionId,
      `${current.original ? "Apply changes to" : "Create"} ${driver === "postgres" ? "role" : "user"} ${userLabel(target)}`,
      () => ipc.admin.applyUser(connectionId, change),
    );
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") {
      setError({ info: outcome.error, executed: outcome.result?.executed ?? [] });
      // Whatever ran before the error (MySQL) is on the server now.
      if ((outcome.result?.executed.length ?? 0) > 0)
        void qc.invalidateQueries({ queryKey: adminKeys.users(connectionId) });
      return;
    }
    toast.success(`${current.original ? "Saved" : "Created"} ${userLabel(target)}`);
    await refresh(userKey(target));
  };

  const drop = async (state: UserState) => {
    setConfirmDrop(null);
    setBusy(true);
    setError(null);
    const outcome = await guarded(
      connectionId,
      `Drop ${driver === "postgres" ? "role" : "user"} ${userLabel(state)}`,
      () => ipc.admin.applyUser(connectionId, dropChange(state)),
    );
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") {
      toast.error(`Could not drop ${userLabel(state)}`, { description: outcome.error.message });
      return;
    }
    toast.success(`Dropped ${userLabel(state)}`);
    await refresh(null);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && (e.key === "s" || e.key === "Enter")) {
      e.preventDefault();
      void apply();
    }
  };

  const problems = current ? draftProblems(current, driver) : [];
  const dropTarget = current?.original ?? null;

  return (
    <div className="flex min-h-0 flex-1" onKeyDown={onKeyDown}>
      <aside className="border-border bg-sidebar flex w-60 shrink-0 flex-col border-r">
        <div className="border-border flex h-9 shrink-0 items-center gap-2 border-b px-3">
          <span className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
            {driver === "postgres" ? "Roles" : "Users"}
          </span>
          <span className="text-muted-foreground/70 text-[11px] tabular-nums">{list.length}</span>
          {users.isFetching && (
            <Loader2 className="text-muted-foreground ml-auto size-3 animate-spin" />
          )}
        </div>
        <ul role="listbox" aria-label="Users" className="min-h-0 flex-1 overflow-y-auto py-1">
          {users.isError && (
            <li className="px-3 py-2">
              <ErrorLine error={errorInfo(users.error)} />
            </li>
          )}
          {selectedKey === NEW_KEY && (
            <li
              role="option"
              aria-selected
              className="bg-brand-soft mx-1 flex h-7 items-center gap-2 rounded-md px-2 text-xs italic"
            >
              <UserRound className="text-muted-foreground size-3.5" />
              {current?.state.name.trim() || "New user"}
            </li>
          )}
          {list.map((u) => (
            <UserListItem
              key={userKey(u)}
              user={u}
              me={isAccount(u, currentUser)}
              selected={userKey(u) === selectedKey}
              onSelect={() => request({ kind: "select", key: userKey(u) })}
              onDrop={() => {
                if (userKey(u) !== selectedKey) request({ kind: "select", key: userKey(u) });
                else if (current?.original) setConfirmDrop(current.original);
              }}
            />
          ))}
        </ul>
        {driver === "postgres" && (
          <label className="border-border flex h-8 shrink-0 items-center gap-2 border-t px-3 text-[11px]">
            <Checkbox
              checked={includeNologin}
              onCheckedChange={(v) => setIncludeNologin(v === true)}
              className="size-3.5"
            />
            Show roles that cannot log in
          </label>
        )}
        <div className="border-border flex h-9 shrink-0 items-center gap-1 border-t px-2">
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={driver === "postgres" ? "New role" : "New user"}
            title={driver === "postgres" ? "New role" : "New user"}
            disabled={busy}
            onClick={() => request({ kind: "new" })}
          >
            <Plus />
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={driver === "postgres" ? "Drop role" : "Drop user"}
            title={driver === "postgres" ? "Drop role…" : "Drop user…"}
            disabled={busy || dropTarget === null}
            onClick={() => dropTarget && setConfirmDrop(dropTarget)}
          >
            <Minus />
          </Button>
        </div>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col">
        {selectedKey === null ? (
          <EmptyState
            icon={UserRound}
            title={driver === "postgres" ? "Select a role" : "Select a user"}
            description="Choose an account on the left, or create one with +."
          />
        ) : !current ? (
          detail.isError ? (
            <EmptyState
              icon={TriangleAlert}
              title="Could not load the account"
              description={errorInfo(detail.error).message}
            />
          ) : (
            <div className="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-xs">
              <Loader2 className="size-3.5 animate-spin" /> Loading…
            </div>
          )
        ) : (
          <UserEditor
            key={draft?.key}
            connectionId={connectionId}
            driver={driver}
            draft={current}
            schemas={detail.data?.schemas ?? []}
            currentDatabase={currentDatabase}
            isMe={current.original !== null && isAccount(current.original, currentUser)}
            onChange={update}
            onPassword={setPassword}
          />
        )}
        {current && (
          <UserFooter
            connectionId={connectionId}
            driver={driver}
            draft={current}
            dirty={dirty}
            problems={problems}
            busy={busy}
            error={error}
            showSql={showSql}
            onToggleSql={() => setShowSql((v) => !v)}
            onDiscard={() => {
              const original = current.original;
              if (original) setDraft((d) => (d ? { ...d, value: draftOf(original) } : d));
              else {
                setSelectedKey(null);
                setDraft(null);
              }
            }}
            onApply={() => void apply()}
          />
        )}
      </section>

      {pending && (
        <AlertDialog open onOpenChange={(open) => !open && setPending(null)}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle className="text-sm">Discard changes?</AlertDialogTitle>
              <AlertDialogDescription className="text-xs">
                The changes to {current ? userLabel(current.state) || "the new account" : "it"} have
                not been applied.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel size="sm">Keep editing</AlertDialogCancel>
              <AlertDialogAction
                size="sm"
                variant="destructive"
                onClick={() => {
                  const next = pending;
                  setPending(null);
                  proceed(next);
                }}
              >
                Discard
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}

      {confirmDrop && (
        <AlertDialog open onOpenChange={(open) => !open && setConfirmDrop(null)}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle className="text-sm">
                Drop {driver === "postgres" ? "role" : "user"} {userLabel(confirmDrop)}?
              </AlertDialogTitle>
              <AlertDialogDescription className="text-xs">
                {driver === "postgres"
                  ? "A role that owns objects or still holds privileges cannot be dropped until they are reassigned or revoked."
                  : "The account and all of its privileges are removed; its open sessions keep running until they disconnect."}{" "}
                This cannot be undone.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
              <Button variant="destructive" size="sm" onClick={() => void drop(confirmDrop)}>
                Drop
              </Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </div>
  );
}

function UserListItem({
  user,
  me,
  selected,
  onSelect,
  onDrop,
}: {
  user: UserSummary;
  me: boolean;
  selected: boolean;
  onSelect: () => void;
  onDrop: () => void;
}) {
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <li
          role="option"
          aria-selected={selected}
          onClick={onSelect}
          onContextMenu={() => {
            if (!selected) onSelect();
          }}
          className={cn(
            "mx-1 flex h-7 cursor-default items-center gap-2 rounded-md px-2 text-xs select-none",
            selected ? "bg-brand-soft" : "hover:bg-muted/60",
          )}
        >
          {user.superuser ? (
            <ShieldCheck
              className="text-muted-foreground size-3.5 shrink-0"
              aria-label="Superuser"
            />
          ) : (
            <UserRound className="text-muted-foreground size-3.5 shrink-0" />
          )}
          <span className={cn("min-w-0 truncate", !user.canLogin && "text-muted-foreground")}>
            {user.name || <span className="italic">(anonymous)</span>}
          </span>
          {user.host !== null && (
            <span className="text-muted-foreground min-w-0 shrink-[2] truncate">@{user.host}</span>
          )}
          {me && (
            <span
              className="text-brand ml-auto shrink-0 text-[10px] font-medium"
              title="This connection's account"
            >
              you
            </span>
          )}
        </li>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem variant="destructive" onSelect={onDrop}>
          Drop {user.host !== null ? "User" : "Role"}…
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

// --- the editor ------------------------------------------------------------------------------

function UserEditor({
  connectionId,
  driver,
  draft,
  schemas,
  currentDatabase,
  isMe,
  onChange,
  onPassword,
}: {
  connectionId: string;
  driver: Driver;
  draft: UserDraft;
  schemas: string[];
  currentDatabase: string | null;
  isMe: boolean;
  onChange: (state: UserState) => void;
  onPassword: (password: string | null) => void;
}) {
  const id = useId();
  const s = draft.state;
  const [reveal, setReveal] = useState(false);
  const mysql = driver === "mysql";
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3">
      <div className={cn("grid gap-3", mysql ? "grid-cols-[1fr_10rem_1fr]" : "grid-cols-2")}>
        <Field label={mysql ? "User name" : "Role name"} htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            autoFocus={draft.original === null}
            value={s.name}
            onChange={(e) => onChange({ ...s, name: e.target.value })}
            placeholder={mysql ? "new_user" : "new_role"}
            className="h-7 font-mono text-xs md:text-xs"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        {mysql && (
          <Field label="Host" htmlFor={`${id}-host`}>
            <Input
              id={`${id}-host`}
              value={s.host ?? ""}
              onChange={(e) => onChange({ ...s, host: e.target.value })}
              placeholder="%"
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
        )}
        <Field label="Password" htmlFor={`${id}-password`}>
          <div className="relative">
            <Input
              id={`${id}-password`}
              type={reveal ? "text" : "password"}
              value={draft.password ?? ""}
              onChange={(e) => onPassword(e.target.value === "" ? null : e.target.value)}
              placeholder={draft.original ? "Unchanged" : "No password"}
              className="h-7 pr-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="new-password"
            />
            <button
              type="button"
              aria-label={reveal ? "Hide password" : "Show password"}
              onClick={() => setReveal((v) => !v)}
              className="text-muted-foreground hover:text-foreground absolute top-1/2 right-1.5 -translate-y-1/2"
            >
              {reveal ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
            </button>
          </div>
        </Field>
      </div>
      {isMe && (
        <p className="text-muted-foreground text-[11px]">
          This is the account this connection uses: removing its privileges also limits Tabler.
        </p>
      )}

      <Tabs defaultValue={mysql ? "global" : "role"} className="min-h-0 flex-1">
        <TabsList className="h-7">
          {mysql ? (
            <TabsTrigger value="global" className="px-2.5 text-xs">
              Global Privileges
            </TabsTrigger>
          ) : (
            <TabsTrigger value="role" className="px-2.5 text-xs">
              Role Attributes
            </TabsTrigger>
          )}
          <TabsTrigger value="database" className="px-2.5 text-xs">
            Database Privileges
          </TabsTrigger>
          {mysql && (
            <TabsTrigger value="limits" className="px-2.5 text-xs">
              Resource Limits
            </TabsTrigger>
          )}
        </TabsList>
        {mysql ? (
          <TabsContent value="global" className="min-h-0">
            <PrivilegeGrid
              label="Global privileges"
              all={MYSQL_GLOBAL_PRIVILEGES}
              granted={s.globalPrivileges}
              onChange={(list) => onChange({ ...s, globalPrivileges: list })}
            />
          </TabsContent>
        ) : (
          <TabsContent value="role" className="min-h-0">
            <RoleAttributesForm
              attributes={s.attributes ?? null}
              onChange={(attributes) => onChange({ ...s, attributes })}
            />
          </TabsContent>
        )}
        <TabsContent value="database" className="min-h-0">
          <DatabasePrivileges
            connectionId={connectionId}
            driver={driver}
            state={s}
            schemas={schemas}
            currentDatabase={currentDatabase}
            onChange={onChange}
          />
        </TabsContent>
        {mysql && (
          <TabsContent value="limits" className="min-h-0">
            <LimitsForm
              limits={s.limits ?? null}
              onChange={(limits) => onChange({ ...s, limits })}
            />
          </TabsContent>
        )}
      </Tabs>
    </div>
  );
}

/** Checkbox list with Check all / None, in three columns. */
function PrivilegeGrid({
  label,
  all,
  granted,
  onChange,
}: {
  label: string;
  all: readonly string[];
  granted: readonly string[];
  onChange: (privileges: string[]) => void;
}) {
  const set = new Set(granted);
  const toggle = (p: string, on: boolean) =>
    onChange(all.filter((x) => (x === p ? on : set.has(x))));
  return (
    <div className="flex flex-col gap-2" role="group" aria-label={label}>
      <div className="flex items-center gap-1">
        <Button variant="ghost" size="xs" onClick={() => onChange([...all])}>
          Check all
        </Button>
        <Button variant="ghost" size="xs" onClick={() => onChange([])}>
          None
        </Button>
        <span className="text-muted-foreground ml-auto text-[11px] tabular-nums">
          {all.filter((p) => set.has(p)).length} of {all.length}
        </span>
      </div>
      <div className="grid grid-cols-3 gap-x-4 gap-y-1.5">
        {all.map((p) => (
          <label key={p} className="flex items-center gap-2 text-xs">
            <Checkbox
              checked={set.has(p)}
              onCheckedChange={(v) => toggle(p, v === true)}
              className="size-3.5"
            />
            <span className="truncate">{p}</span>
          </label>
        ))}
      </div>
    </div>
  );
}

const ROLE_FLAGS: Array<{ key: keyof RoleAttributes; label: string; hint: string }> = [
  { key: "login", label: "Can log in", hint: "LOGIN: a user rather than a group role" },
  { key: "superuser", label: "Superuser", hint: "SUPERUSER: bypasses every permission check" },
  { key: "createDb", label: "Create databases", hint: "CREATEDB" },
  { key: "createRole", label: "Create roles", hint: "CREATEROLE" },
  { key: "replication", label: "Replication", hint: "REPLICATION: may stream WAL" },
  { key: "inherit", label: "Inherit privileges", hint: "INHERIT: of the roles it is a member of" },
];

function RoleAttributesForm({
  attributes,
  onChange,
}: {
  attributes: RoleAttributes | null;
  onChange: (attributes: RoleAttributes) => void;
}) {
  const id = useId();
  if (!attributes) return null;
  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-2 gap-x-6 gap-y-2">
        {ROLE_FLAGS.map((f) => (
          <label key={f.key} className="flex items-start gap-2 text-xs">
            <Checkbox
              checked={attributes[f.key] === true}
              onCheckedChange={(v) => onChange({ ...attributes, [f.key]: v === true })}
              className="mt-px size-3.5"
            />
            <span className="flex flex-col">
              {f.label}
              <span className="text-muted-foreground text-[11px]">{f.hint}</span>
            </span>
          </label>
        ))}
      </div>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Connection limit" htmlFor={`${id}-limit`} hint="-1 = no limit">
          <Input
            id={`${id}-limit`}
            type="number"
            min={-1}
            step={1}
            value={Number.isNaN(attributes.connectionLimit) ? "" : attributes.connectionLimit}
            onChange={(e) =>
              onChange({
                ...attributes,
                connectionLimit: e.target.value === "" ? -1 : Number(e.target.value),
              })
            }
            className="h-7 text-xs tabular-nums md:text-xs"
          />
        </Field>
        <Field
          label="Valid until"
          htmlFor={`${id}-valid`}
          hint="Empty = the password never expires"
        >
          <Input
            id={`${id}-valid`}
            value={attributes.validUntil ?? ""}
            onChange={(e) => onChange({ ...attributes, validUntil: e.target.value || null })}
            placeholder="2027-12-31 23:59:59+00"
            className="h-7 font-mono text-xs md:text-xs"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      </div>
    </div>
  );
}

const LIMIT_FIELDS: Array<{ key: keyof ResourceLimits; label: string }> = [
  { key: "maxQueriesPerHour", label: "Max queries per hour" },
  { key: "maxUpdatesPerHour", label: "Max updates per hour" },
  { key: "maxConnectionsPerHour", label: "Max connections per hour" },
  { key: "maxUserConnections", label: "Max user connections" },
];

function LimitsForm({
  limits,
  onChange,
}: {
  limits: ResourceLimits | null;
  onChange: (limits: ResourceLimits) => void;
}) {
  const id = useId();
  if (!limits) return null;
  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-2 gap-3">
        {LIMIT_FIELDS.map((f) => (
          <Field key={f.key} label={f.label} htmlFor={`${id}-${f.key}`}>
            <Input
              id={`${id}-${f.key}`}
              type="number"
              min={0}
              step={1}
              value={limits[f.key]}
              onChange={(e) =>
                onChange({ ...limits, [f.key]: e.target.value === "" ? 0 : Number(e.target.value) })
              }
              className="h-7 text-xs tabular-nums md:text-xs"
            />
          </Field>
        ))}
      </div>
      <p className="text-muted-foreground text-[11px]">0 = unlimited.</p>
    </div>
  );
}

/** A tri-state box for "on all tables of the schema": all ✓, some −, none. */
function TriCheckbox({
  state,
  label,
  onChange,
}: {
  state: "all" | "some" | "none";
  label: string;
  onChange: (next: "all" | "none") => void;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={state === "all" ? true : state === "some" ? "mixed" : false}
      aria-label={label}
      title={state === "some" ? "Granted on some tables only" : undefined}
      onClick={() => onChange(state === "all" ? "none" : "all")}
      className={cn(
        "border-input focus-visible:ring-ring/50 mx-auto flex size-3.5 items-center justify-center rounded-[4px] border outline-none focus-visible:ring-3",
        state !== "none" && "border-primary bg-primary text-primary-foreground",
        state === "some" && "opacity-70",
      )}
    >
      {state === "all" && <Check className="size-3" />}
      {state === "some" && <Minus className="size-3" />}
    </button>
  );
}

function DatabasePrivileges({
  connectionId,
  driver,
  state,
  schemas,
  currentDatabase,
  onChange,
}: {
  connectionId: string;
  driver: Driver;
  state: UserState;
  schemas: string[];
  currentDatabase: string | null;
  onChange: (state: UserState) => void;
}) {
  const id = useId();
  const databases = useDatabases(connectionId);
  const choices = useMemo(
    () =>
      privilegeDatabases(
        (databases.data ?? []).map((d) => d.name),
        state,
      ),
    [databases.data, state],
  );
  const [picked, setPicked] = useState<string | null>(null);
  const database =
    picked ?? (choices.includes(currentDatabase ?? "") ? currentDatabase : (choices[0] ?? null));
  const mysql = driver === "mysql";
  const order: readonly string[] = mysql ? MYSQL_DB_PRIVILEGES : PG_DATABASE_PRIVILEGES;
  const granted = state.databasePrivileges.filter((p) => p.privileges.length > 0);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-end gap-3">
        <Field label="Database" htmlFor={`${id}-db`} className="w-64">
          <NativeSelect
            id={`${id}-db`}
            value={database ?? ""}
            onChange={(e) => setPicked(e.target.value)}
          >
            {database === null && (
              <option value="">{databases.isPending ? "Loading…" : "No databases"}</option>
            )}
            {choices.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </NativeSelect>
        </Field>
      </div>
      {database !== null &&
        (mysql ? (
          <PrivilegeGrid
            label={`Privileges on ${database}`}
            all={MYSQL_DB_PRIVILEGES}
            granted={privilegesOn(state.databasePrivileges, database)}
            onChange={(list) =>
              onChange({
                ...state,
                databasePrivileges: setPrivileges(state.databasePrivileges, database, list, order),
              })
            }
          />
        ) : (
          <div className="flex gap-4" role="group" aria-label={`Privileges on ${database}`}>
            {PG_DATABASE_PRIVILEGES.map((p) => (
              <label key={p} className="flex items-center gap-2 text-xs">
                <Checkbox
                  checked={privilegesOn(state.databasePrivileges, database).includes(p)}
                  onCheckedChange={(v) =>
                    onChange({
                      ...state,
                      databasePrivileges: setPrivilege(
                        state.databasePrivileges,
                        database,
                        p,
                        v === true,
                        order,
                      ),
                    })
                  }
                  className="size-3.5"
                />
                {p}
              </label>
            ))}
          </div>
        ))}
      {!mysql &&
        database !== null &&
        (database === currentDatabase ? (
          <SchemaPrivileges state={state} schemas={schemas} onChange={onChange} />
        ) : (
          <p className="text-muted-foreground text-[11px]">
            Schema and table privileges of {database} can be edited while this connection uses it
            (switch with {formatCombo(shortcutFor("palette.databases") ?? "Mod+K")}).
          </p>
        ))}
      <div className="flex flex-col gap-1">
        <h4 className="text-muted-foreground text-[11px] font-medium">Granted Privileges</h4>
        {granted.length === 0 ? (
          <p className="text-muted-foreground/70 text-[11px] italic">None on any database</p>
        ) : (
          <ul className="border-border divide-border divide-y rounded-md border text-xs">
            {granted.map((p) => (
              <li key={p.object}>
                <button
                  type="button"
                  onClick={() => setPicked(p.object)}
                  className={cn(
                    "hover:bg-muted/50 flex w-full min-w-0 items-baseline gap-2 px-2 py-1 text-left",
                    p.object === database && "bg-brand-soft",
                  )}
                >
                  <span className="shrink-0 font-mono">{p.object}</span>
                  <span className="text-muted-foreground truncate text-[11px]">
                    {p.privileges.join(", ")}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

/** PG: USAGE / CREATE on each schema, and SELECT / INSERT / UPDATE / DELETE on all its tables. */
function SchemaPrivileges({
  state,
  schemas,
  onChange,
}: {
  state: UserState;
  schemas: string[];
  onChange: (state: UserState) => void;
}) {
  if (schemas.length === 0)
    return <p className="text-muted-foreground text-[11px]">This database has no schemas.</p>;
  return (
    <div className="border-border overflow-auto rounded-md border">
      <table className="w-full border-collapse text-xs">
        <thead>
          <tr className="text-muted-foreground bg-surface-2 h-7 text-[11px]">
            <th className="px-2 text-left font-medium">Schema</th>
            {PG_SCHEMA_PRIVILEGES.map((p) => (
              <th key={p} className="w-16 px-1 font-medium">
                {p}
              </th>
            ))}
            {PG_TABLE_PRIVILEGES.map((p, i) => (
              <th
                key={p}
                className={cn("w-16 px-1 font-medium", i === 0 && "border-border border-l")}
                title={`${p} on all tables of the schema`}
              >
                {p}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {schemas.map((schema) => (
            <tr key={schema} className="border-gridline h-7 border-t">
              <td className="truncate px-2 font-mono">{schema}</td>
              {PG_SCHEMA_PRIVILEGES.map((p) => (
                <td key={p} className="px-1 text-center">
                  <Checkbox
                    aria-label={`${p} on schema ${schema}`}
                    checked={privilegesOn(state.schemaPrivileges, schema).includes(p)}
                    onCheckedChange={(v) =>
                      onChange({
                        ...state,
                        schemaPrivileges: setPrivilege(
                          state.schemaPrivileges,
                          schema,
                          p,
                          v === true,
                          PG_SCHEMA_PRIVILEGES,
                        ),
                      })
                    }
                    className="mx-auto size-3.5"
                  />
                </td>
              ))}
              {PG_TABLE_PRIVILEGES.map((p, i) => (
                <td key={p} className={cn("px-1 text-center", i === 0 && "border-border border-l")}>
                  <TriCheckbox
                    label={`${p} on all tables in ${schema}`}
                    state={tablePrivilegeState(state, schema, p)}
                    onChange={(next) => onChange(setTablePrivilege(state, schema, p, next))}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-muted-foreground border-border border-t px-2 py-1 text-[11px]">
        Table privileges cover the tables and views that exist now (GRANT … ON ALL TABLES IN
        SCHEMA); − = granted on some tables only.
      </p>
    </div>
  );
}

function UserFooter({
  connectionId,
  driver,
  draft,
  dirty,
  problems,
  busy,
  error,
  showSql,
  onToggleSql,
  onDiscard,
  onApply,
}: {
  connectionId: string;
  driver: Driver;
  draft: UserDraft;
  dirty: boolean;
  problems: string[];
  busy: boolean;
  error: { info: AdminErrorInfo; executed: string[] } | null;
  showSql: boolean;
  onToggleSql: () => void;
  onDiscard: () => void;
  onApply: () => void;
}) {
  // The preview never needs the password itself: the backend masks it anyway, so a stand-in
  // keeps it out of the query cache.
  const change = useMemo(() => {
    const c = toChange(draft);
    return { ...c, password: c.password === null ? null : "********" };
  }, [draft]);
  const preview = useQuery({
    queryKey: ["adminPreviewUser", connectionId, change],
    queryFn: () => ipc.admin.previewUser(connectionId, change),
    enabled: showSql && dirty && problems.length === 0,
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    retry: false,
  });
  return (
    <div className="border-border shrink-0 border-t">
      {showSql && (
        <div className="border-border bg-surface-2 max-h-44 min-h-10 overflow-auto border-b px-3 py-2">
          {!dirty ? (
            <p className="text-muted-foreground text-[11px]">No changes.</p>
          ) : problems.length > 0 ? (
            <p className="text-muted-foreground text-[11px]">{problems[0]}</p>
          ) : preview.isError ? (
            <ErrorLine error={errorInfo(preview.error)} />
          ) : preview.data ? (
            <SqlBlock
              sql={preview.data.length > 0 ? `${preview.data.join(";\n")};` : "-- nothing to run"}
              dialect={driver}
            />
          ) : (
            <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
          )}
        </div>
      )}
      {error && (
        <div className="border-border border-b px-3 py-2">
          <ErrorLine error={error.info} />
          {error.executed.length > 0 && (
            <p className="text-muted-foreground mt-1 text-[11px]">
              {error.executed.length} statement{error.executed.length === 1 ? "" : "s"} ran before
              the error and stay applied.
            </p>
          )}
        </div>
      )}
      <div className="flex h-11 items-center gap-2 px-3">
        <Button variant="ghost" size="xs" aria-expanded={showSql} onClick={onToggleSql}>
          <ChevronRight className={cn("transition-transform", showSql && "rotate-90")} />
          Preview SQL
        </Button>
        {problems.length > 0 && dirty && (
          <span className="text-muted-foreground truncate text-[11px]">{problems[0]}</span>
        )}
        <span className="flex-1" />
        <Button variant="ghost" size="sm" disabled={busy || !dirty} onClick={onDiscard}>
          {draft.original ? "Discard" : "Cancel"}
        </Button>
        <Button size="sm" disabled={busy || !dirty || problems.length > 0} onClick={onApply}>
          {busy && <Loader2 className="animate-spin" />}
          {draft.original ? "Apply" : "Create"}
        </Button>
      </div>
    </div>
  );
}
