import { toggleLineComment } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { FileCode2, FolderOpen, Info, Loader2, Save } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  editorPrefsOf,
  prefsConf,
  prefsExtension,
  toggleBlockCommentSmart,
} from "@/features/editor/cmExtensions";
import { changeEditorFontSize } from "@/features/editor/editorSettings";
import { baseName } from "@/features/editor/files";
import type { SqlDialect } from "@/features/editor/scanner";
import { useAction, type ActionId } from "@/features/palette/actions";
import { shortcutFor, useKeymapVersion } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { getPrefs, usePrefsStore } from "@/stores/prefs";
import {
  focusViewer,
  getViewerView,
  registerViewerView,
  setViewerDialect,
  stashViewerState,
  takeViewerState,
} from "./buffers";
import { loadViewerFile, pickViewerFile, saveViewer } from "./files";
import { DIALECT_LABELS, DIALECTS, viewerTitle } from "./labels";
import { getDoc, patchDoc, useSqlViewerStore } from "./store";

function Tip({
  label,
  action,
  children,
}: {
  label: string;
  action?: ActionId;
  children: ReactNode;
}) {
  useKeymapVersion();
  const combo = action ? shortcutFor(action) : undefined;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent className="flex items-center gap-1.5">
        {label}
        {combo && <Kbd combo={combo} className="border-border/40 bg-transparent text-inherit" />}
      </TooltipContent>
    </Tooltip>
  );
}

/** A one-line question above the editor (unsaved changes before a file replaces them…). */
export function ViewerNotice({
  message,
  actions,
}: {
  message: string;
  actions: { label: string; run: () => void; primary?: boolean }[];
}) {
  return (
    <div
      role="alert"
      className="bg-warning/10 text-foreground border-border flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-xs"
    >
      <span className="min-w-0 flex-1 truncate">{message}</span>
      {actions.map((a) => (
        <Button
          key={a.label}
          size="xs"
          variant={a.primary ? "default" : "ghost"}
          className="h-6 text-[11px]"
          onClick={a.run}
        >
          {a.label}
        </Button>
      ))}
    </div>
  );
}

export interface SqlViewerProps {
  /** The viewer's key (its tab id, or `STANDALONE`). */
  viewerKey: string;
  /** Its shortcuts and menu items are on (the focused tab / the open standalone viewer). */
  active: boolean;
  /**
   * File ▸ Open (⌘O) with a file picked: where it goes. Default: into this viewer, after the user
   * discarded its unsaved changes.
   */
  onOpenPath?: (path: string) => void;
  /** Shown above the editor (the standalone viewer's close confirmation). */
  notice?: ReactNode;
}

/**
 * The SQL Viewer: a CodeMirror SQL editor (highlighting, comments, multiple carets, keyword
 * completion) for one `.sql` file, with Open / Save / Save As and a dialect for the highlighting.
 * Nothing runs: there is no connection behind it.
 */
export function SqlViewer({ viewerKey, active, onOpenPath, notice }: SqlViewerProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const doc = useSqlViewerStore((s) => s.docs[viewerKey]);
  const dialect = doc?.dialect ?? "postgres";
  const dialectRef = useRef(dialect);
  dialectRef.current = dialect;

  // One EditorView per mount; the buffer (text, undo history) outlives it.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const view = new EditorView({
      state: takeViewerState(viewerKey, dialectRef.current),
      parent: host,
    });
    // The preferences may have changed while the viewer was hidden.
    view.dispatch({ effects: prefsConf.reconfigure(prefsExtension(editorPrefsOf(getPrefs()))) });
    const unregister = registerViewerView(viewerKey, view);
    view.focus();
    return () => {
      // A viewer that was closed for good (its doc is gone) keeps nothing.
      if (getDoc(viewerKey)) stashViewerState(viewerKey, view.state);
      unregister();
      view.destroy();
    };
  }, [viewerKey]);

  const prefs = usePrefsStore(useShallow((s) => editorPrefsOf(s.prefs)));
  useEffect(() => {
    getViewerView(viewerKey)?.dispatch({ effects: prefsConf.reconfigure(prefsExtension(prefs)) });
  }, [viewerKey, prefs]);

  const open = useCallback(async () => {
    const path = await pickViewerFile();
    if (!path) return;
    if (onOpenPath) return onOpenPath(path);
    const current = useSqlViewerStore.getState().docs[viewerKey];
    if (current?.dirty) patchDoc(viewerKey, { pendingPath: path });
    else await loadViewerFile(viewerKey, path);
    focusViewer(viewerKey);
  }, [viewerKey, onOpenPath]);
  const save = useCallback(async () => {
    await saveViewer(viewerKey);
    focusViewer(viewerKey);
  }, [viewerKey]);
  const saveAs = useCallback(async () => {
    await saveViewer(viewerKey, { saveAs: true });
    focusViewer(viewerKey);
  }, [viewerKey]);
  const comment = useCallback(
    (kind: "line" | "block") => () => {
      const view = getViewerView(viewerKey);
      if (!view) return;
      if (kind === "line") toggleLineComment(view);
      else toggleBlockCommentSmart(view);
      view.focus();
    },
    [viewerKey],
  );
  const commentLine = useCallback(() => comment("line")(), [comment]);
  const commentBlock = useCallback(() => comment("block")(), [comment]);
  const fontBigger = useCallback(() => changeEditorFontSize(getPrefs().editorFontSize, 1), []);
  const fontSmaller = useCallback(() => changeEditorFontSize(getPrefs().editorFontSize, -1), []);

  // File ▸ Open SQL File… / Save… and the SQL editor's keys act on the viewer while it shows.
  useAction("query.open", active ? open : null);
  useAction("query.save", active ? save : null);
  useAction("query.save-as", active ? saveAs : null);
  useAction("query.comment-line", active ? commentLine : null);
  useAction("query.comment-block", active ? commentBlock : null);
  useAction("query.font-bigger", active ? fontBigger : null);
  useAction("query.font-smaller", active ? fontSmaller : null);

  const changeDialect = (value: string) => {
    const next = value as SqlDialect;
    patchDoc(viewerKey, { dialect: next });
    setViewerDialect(viewerKey, next);
    focusViewer(viewerKey);
  };

  const pending = doc?.pendingPath ?? null;
  const discardAndOpen = () => {
    if (pending) void loadViewerFile(viewerKey, pending);
  };

  return (
    <div className="bg-background flex h-full min-h-0 flex-col" data-slot="sql-viewer">
      <div className="border-border bg-surface flex h-8 shrink-0 items-center gap-1 border-b px-2 text-xs">
        <FileCode2 className="text-muted-foreground size-3.5 shrink-0" aria-hidden />
        <span className="min-w-0 truncate font-medium" title={doc?.path ?? undefined}>
          {viewerTitle(doc)}
        </span>
        {doc?.dirty && (
          <span className="text-muted-foreground shrink-0 text-[11px]" aria-label="Edited">
            — Edited
          </span>
        )}
        {doc?.loading && <Loader2 className="text-brand size-3 shrink-0 animate-spin" />}
        <span className="text-muted-foreground ml-1 hidden shrink-0 text-[11px] sm:inline">
          SQL Viewer · runs nothing
        </span>
        <div className="flex-1" />
        <Select value={dialect} onValueChange={changeDialect}>
          <SelectTrigger
            size="sm"
            aria-label="SQL dialect"
            className={cn(
              "text-muted-foreground hover:text-foreground hover:bg-muted dark:hover:bg-muted/50 gap-1 border-transparent bg-transparent px-1.5 text-[11px] shadow-none data-[size=sm]:h-6 dark:bg-transparent [&_svg]:size-3",
              "data-[state=open]:bg-muted data-[state=open]:text-foreground",
            )}
          >
            <SelectValue>{DIALECT_LABELS[dialect]}</SelectValue>
          </SelectTrigger>
          <SelectContent align="end" className="min-w-28">
            {DIALECTS.map((d) => (
              <SelectItem key={d} value={d} className="text-xs">
                {DIALECT_LABELS[d]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Tip label="Open a .sql file" action="query.open">
          <Button
            size="xs"
            variant="ghost"
            className="h-6 gap-1 px-1.5 text-[11px]"
            onClick={() => void open()}
          >
            <FolderOpen className="size-3" />
            Open…
          </Button>
        </Tip>
        <Tip label="Save the file" action="query.save">
          <Button
            size="xs"
            variant="ghost"
            className="h-6 gap-1 px-1.5 text-[11px]"
            onClick={() => void save()}
          >
            <Save className="size-3" />
            Save
          </Button>
        </Tip>
        <Button
          size="xs"
          variant="ghost"
          className="h-6 px-1.5 text-[11px]"
          onClick={() => void saveAs()}
        >
          Save As…
        </Button>
      </div>
      {notice}
      {doc?.headNotice && (
        <div
          role="status"
          data-testid="head-notice"
          className="border-border bg-muted/50 text-muted-foreground flex shrink-0 items-center gap-1.5 border-b px-3 py-1 text-[11px]"
        >
          <Info aria-hidden className="size-3 shrink-0" />
          <span className="min-w-0 truncate">{doc.headNotice}</span>
        </div>
      )}
      {pending && (
        <ViewerNotice
          message={`Discard the unsaved changes to ${viewerTitle(doc)} and open ${baseName(pending)}?`}
          actions={[
            { label: "Cancel", run: () => patchDoc(viewerKey, { pendingPath: null }) },
            { label: "Discard and Open", run: discardAndOpen, primary: true },
          ]}
        />
      )}
      {doc?.error && !doc.loading && (
        <div className="text-destructive border-border shrink-0 border-b px-3 py-1.5 text-xs">
          {doc.error}
        </div>
      )}
      <div
        ref={hostRef}
        className="min-h-0 flex-1 overflow-hidden"
        data-testid="sql-viewer-editor"
      />
    </div>
  );
}
