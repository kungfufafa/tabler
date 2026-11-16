import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { FormError } from "@/components/FormError";
import { FormField } from "@/components/FormField";
import { KbdHint } from "@/components/KbdHint";
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
import { usePrefs } from "@/stores/prefs";
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
/** "Auto Save Favorite": quiet time after the last edit before it is saved. */
export const FAVORITE_AUTOSAVE_MS = 600;

const sameDraft = (a: SavedQueryDraft, b: SavedQueryDraft) =>
  a.name === b.name &&
  a.keyword === b.keyword &&
  a.sql === b.sql &&
  a.folderId === b.folderId &&
  a.profileId === b.profileId;

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

  // "Auto Save Favorite": an existing query saves shortly after each edit (⌘S still closes).
  const autoSave = usePrefs((p) => p.queriesAutoSave) && !isNew;
  const lastSaved = useRef(request.draft);
  const [autoStatus, setAutoStatus] = useState<"saved" | "saving" | null>(null);
  useEffect(() => {
    if (!autoSave || sameDraft(draft, lastSaved.current)) return;
    if (draft.name.trim() === "" || keywordProblem(draft.keyword) !== null) return;
    const timer = setTimeout(() => {
      const snapshot = draft;
      setAutoStatus("saving");
      void putSavedQuery({
        id: snapshot.id,
        name: snapshot.name.trim(),
        keyword: snapshot.keyword.trim() || null,
        sql: snapshot.sql,
        folderId: snapshot.folderId,
        profileId: snapshot.profileId,
      }).then(
        (saved) => {
          lastSaved.current = snapshot;
          setAutoStatus("saved");
          request.onSaved?.(saved);
        },
        (err: unknown) => {
          setAutoStatus(null);
          setError(toIpcError(err).message);
        },
      );
    }, FAVORITE_AUTOSAVE_MS);
    return () => clearTimeout(timer);
  }, [autoSave, draft, request]);

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
    <DialogContent className="sm:max-w-[34rem]" onKeyDown={onKeyDown}>
      <DialogHeader>
        <DialogTitle className="text-sm">{isNew ? "Add to Queries" : "Edit Query"}</DialogTitle>
        <DialogDescription className="text-xs">
          Saved queries are listed in the sidebar's Queries pane. Type the keyword in the SQL editor
          to insert the query.
        </DialogDescription>
      </DialogHeader>
      <form className="flex flex-col gap-3" onSubmit={(e) => void save(e)}>
        <FormField orientation="inline" htmlFor="saved-query-name" label="Name">
          <Input
            id="saved-query-name"
            autoFocus
            onFocus={(e) => e.currentTarget.select()}
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="Untitled Query"
            autoComplete="off"
            className="h-7 rounded-md text-xs md:text-xs"
          />
        </FormField>
        <FormField
          orientation="inline"
          htmlFor="saved-query-keyword"
          label="Keyword"
          error={keywordError}
          hint={
            sharedWith
              ? `“${sharedWith.name}” has this keyword too: both are offered.`
              : "Typing it in the SQL editor offers this query."
          }
        >
          <Input
            id="saved-query-keyword"
            value={draft.keyword}
            onChange={(e) => set({ keyword: e.target.value })}
            placeholder="Optional, e.g. recent"
            aria-invalid={keywordError !== null}
            aria-describedby={
              keywordError !== null ? "saved-query-keyword-error" : "saved-query-keyword-hint"
            }
            spellCheck={false}
            autoComplete="off"
            className="h-7 rounded-md font-mono text-xs md:text-xs"
          />
        </FormField>
        <FormField orientation="inline" htmlFor="saved-query-sql" label="SQL">
          <Textarea
            id="saved-query-sql"
            value={draft.sql}
            onChange={(e) => set({ sql: e.target.value })}
            spellCheck={false}
            rows={10}
            className="max-h-[40dvh] min-h-40 resize-y rounded-md font-mono text-xs leading-5 md:text-xs"
          />
        </FormField>
        <div className="grid grid-cols-2 gap-3">
          <FormField orientation="inline" htmlFor="saved-query-folder" label="Folder">
            <Select
              value={draft.folderId ?? TOP}
              onValueChange={(v) => set({ folderId: v === TOP ? null : v })}
            >
              <SelectTrigger id="saved-query-folder" size="sm" className="w-full text-xs">
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
          </FormField>
          <FormField orientation="inline" htmlFor="saved-query-scope" label="Connection">
            <Select
              value={draft.profileId ?? ALL}
              onValueChange={(v) => set({ profileId: v === ALL ? null : v })}
              disabled={scopeProfile === null}
            >
              <SelectTrigger id="saved-query-scope" size="sm" className="w-full text-xs">
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
          </FormField>
        </div>
        {error && <FormError>{error}</FormError>}
        {/* ↩ in a single-line field saves. */}
        <button type="submit" hidden aria-hidden tabIndex={-1} />
      </form>
      <DialogFooter className="items-center">
        {autoSave && (
          <span className="text-muted-foreground mr-auto text-xs" role="status">
            {autoStatus === "saving"
              ? "Saving…"
              : autoStatus === "saved"
                ? "Saved automatically"
                : "Changes save automatically"}
          </span>
        )}
        <Button size="sm" variant="outline" onClick={closeSavedQueryDialog}>
          {autoSave && autoStatus === "saved" ? "Close" : "Cancel"}
        </Button>
        <Button size="sm" onClick={() => void save()} disabled={!canSave}>
          Save
          <KbdHint combo="Mod+S" />
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
