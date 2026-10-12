import { useMemo, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { toIpcError } from "@/lib/ipc";
import {
  closeSavedQueryDialog,
  keywordProblem,
  useSavedQueryDialogStore,
  type SavedQueryDialogRequest,
  type SavedQueryDraft,
} from "./dialogStore";
import { putSavedQuery, queriesFor, useSavedQueriesStore } from "./store";
import { folderOptions } from "./tree";

const ALL = "__all__";
const TOP = "__top__";

function Field({
  id,
  label,
  hint,
  children,
}: {
  id: string;
  label: string;
  hint?: string | null;
  children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[5.5rem_1fr] items-start gap-x-3 gap-y-1">
      <label htmlFor={id} className="text-muted-foreground pt-1.5 text-right text-xs">
        {label}
      </label>
      <div className="flex min-w-0 flex-col gap-1">
        {children}
        {hint && <p className="text-muted-foreground text-[11px] leading-4">{hint}</p>}
      </div>
    </div>
  );
}

function SavedQueryForm({ request }: { request: SavedQueryDialogRequest }) {
  const [draft, setDraft] = useState<SavedQueryDraft>(request.draft);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const folders = useSavedQueriesStore((s) => s.folders);
  const queries = useSavedQueriesStore((s) => s.queries);
  const options = useMemo(() => folderOptions(folders), [folders]);
  const set = (patch: Partial<SavedQueryDraft>) => {
    setError(null);
    setDraft((d) => ({ ...d, ...patch }));
  };
  const isNew = draft.id === null;
  const { connection } = request;
  // The connection choice: every connection, or the one it was opened from (or kept to).
  const scopeProfile = draft.profileId ?? connection?.profileId ?? null;
  const scopeName =
    connection && connection.profileId === scopeProfile ? connection.name : "this connection";

  const keywordError = keywordProblem(draft.keyword);
  const keyword = draft.keyword.trim();
  const sharedWith = keyword
    ? queriesFor(queries, draft.profileId ?? connection?.profileId).find(
        (q) => q.id !== draft.id && q.keyword?.toLowerCase() === keyword.toLowerCase(),
      )
    : undefined;
  const canSave = draft.name.trim() !== "" && keywordError === null && !saving;

  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!canSave) return;
    setSaving(true);
    try {
      const saved = await putSavedQuery({
        id: draft.id,
        name: draft.name.trim(),
        keyword: keyword || null,
        sql: draft.sql,
        folderId: draft.folderId,
        profileId: draft.profileId,
      });
      closeSavedQueryDialog();
      toast.success(isNew ? `Added “${saved.name}” to Queries` : `Saved “${saved.name}”`);
      request.onSaved?.(saved);
    } catch (err) {
      setError(toIpcError(err).message);
      setSaving(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    // ⌘S saves (TablePlus); ⌘↩ too.
    if (mod && !e.altKey && !e.shiftKey && (e.key.toLowerCase() === "s" || e.key === "Enter")) {
      e.preventDefault();
      e.stopPropagation();
      void save();
    }
  };

  return (
    <DialogContent className="gap-3 sm:max-w-xl" onKeyDown={onKeyDown}>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-sm">{isNew ? "Add to Queries" : "Edit Query"}</DialogTitle>
        <DialogDescription className="text-xs">
          Saved queries are listed in the sidebar's Queries pane. Type the keyword in the SQL editor
          to insert the query.
        </DialogDescription>
      </DialogHeader>
      <form className="flex flex-col gap-2.5" onSubmit={(e) => void save(e)}>
        <Field id="saved-query-name" label="Name">
          <Input
            id="saved-query-name"
            autoFocus
            onFocus={(e) => e.currentTarget.select()}
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="Untitled Query"
            autoComplete="off"
            className="h-7 text-xs"
          />
        </Field>
        <Field
          id="saved-query-keyword"
          label="Keyword"
          hint={
            keywordError ??
            (sharedWith
              ? `“${sharedWith.name}” has this keyword too: both are offered.`
              : "Optional. Typing it in the SQL editor offers this query.")
          }
        >
          <Input
            id="saved-query-keyword"
            value={draft.keyword}
            onChange={(e) => set({ keyword: e.target.value })}
            placeholder="e.g. recent"
            aria-invalid={keywordError !== null}
            spellCheck={false}
            autoComplete="off"
            className="h-7 font-mono text-xs"
          />
        </Field>
        <Field id="saved-query-sql" label="SQL">
          <Textarea
            id="saved-query-sql"
            value={draft.sql}
            onChange={(e) => set({ sql: e.target.value })}
            spellCheck={false}
            rows={10}
            className="max-h-[40vh] min-h-40 resize-y font-mono text-xs leading-5 md:text-xs"
          />
        </Field>
        <div className="grid grid-cols-2 gap-x-3">
          <Field id="saved-query-folder" label="Folder">
            <Select
              value={draft.folderId ?? TOP}
              onValueChange={(v) => set({ folderId: v === TOP ? null : v })}
            >
              <SelectTrigger id="saved-query-folder" size="sm" className="h-7 w-full text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent data-scope="dialog">
                <SelectItem value={TOP} className="text-xs">
                  None
                </SelectItem>
                {options.map(({ folder, depth }) => (
                  <SelectItem key={folder.id} value={folder.id} className="text-xs">
                    <span style={{ paddingLeft: depth * 10 }}>{folder.name}</span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Field id="saved-query-scope" label="Connection">
            <Select
              value={draft.profileId ?? ALL}
              onValueChange={(v) => set({ profileId: v === ALL ? null : v })}
              disabled={scopeProfile === null}
            >
              <SelectTrigger id="saved-query-scope" size="sm" className="h-7 w-full text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent data-scope="dialog">
                <SelectItem value={ALL} className="text-xs">
                  All connections
                </SelectItem>
                {scopeProfile !== null && (
                  <SelectItem value={scopeProfile} className="text-xs">
                    Only {scopeName}
                  </SelectItem>
                )}
              </SelectContent>
            </Select>
          </Field>
        </div>
        {error && (
          <p role="alert" className="text-error text-xs">
            {error}
          </p>
        )}
        {/* ↩ in a single-line field saves. */}
        <button type="submit" hidden aria-hidden tabIndex={-1} />
      </form>
      <DialogFooter className="py-3">
        <Button size="sm" variant="outline" onClick={closeSavedQueryDialog}>
          Cancel
        </Button>
        <Button size="sm" onClick={() => void save()} disabled={!canSave}>
          Save
          <Kbd combo="Mod+S" className="border-primary-foreground/30 bg-transparent text-inherit" />
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

/** The saved query popup (Add to Queries / Edit), opened from the editor, History or Queries. */
export function SavedQueryDialog() {
  const request = useSavedQueryDialogStore((s) => s.request);
  return (
    <Dialog open={request !== null} onOpenChange={(open) => !open && closeSavedQueryDialog()}>
      {request && <SavedQueryForm key={request.seq} request={request} />}
    </Dialog>
  );
}
