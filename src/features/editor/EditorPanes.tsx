import { toggleLineComment } from "@codemirror/commands";
import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { X } from "lucide-react";
import { useEffect, type ReactNode } from "react";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { readClipboardText, writeClipboard } from "@/features/datagrid/clipboard";
import type { ActionId } from "@/features/palette/actions";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { toggleBlockCommentSmart } from "./cmExtensions";
import type { EditorBarHandlers } from "./EditorBar";
import { dropEditor, getEditorView } from "./editorStates";
import { EditorMenuExtensions } from "./menuExtensions";
import type { EditorLanguage } from "./language";
import {
  isSplitOpen,
  setFocusedPane,
  splitKey,
  useActiveEditorKey,
  useSplitOpen,
  type EditorPane,
} from "./split";
import { SqlEditor } from "./SqlEditor";

export interface EditorPaneHandlers extends EditorBarHandlers {
  /** Splits the editor in two (⇧⌘D), or closes the second pane. */
  split: () => void;
}

function Shortcut({ action, combo }: { action?: ActionId; combo?: string }) {
  const keys = combo ?? (action ? shortcutFor(action) : undefined);
  return keys ? <ContextMenuShortcut>{formatCombo(keys)}</ContextMenuShortcut> : null;
}

/** The text of every selection range, one per line. */
function selectionText(view: EditorView): string {
  return view.state.selection.ranges
    .filter((r) => !r.empty)
    .map((r) => view.state.sliceDoc(r.from, r.to))
    .join("\n");
}

/**
 * Right-click menu of an editor pane: run, the clipboard, comments, formatting, "Add to
 * Queries…", saving the selection and the split pane. It replaces the webview's own menu, so it
 * carries Cut / Copy / Paste / Select All itself.
 */
function EditorContextMenu({
  tabId,
  editorKey,
  split,
  handlers,
  children,
}: {
  tabId: string;
  editorKey: string;
  split: boolean;
  handlers: EditorPaneHandlers;
  children: ReactNode;
}) {
  const withView = (fn: (view: EditorView) => void) => () => {
    const view = getEditorView(editorKey);
    if (view) fn(view);
  };
  const copy = withView((view) => {
    const text = selectionText(view);
    if (text) void writeClipboard(text);
  });
  const cut = withView((view) => {
    const text = selectionText(view);
    if (!text) return;
    void writeClipboard(text);
    view.dispatch(view.state.replaceSelection(""), { userEvent: "delete.cut" });
  });
  const paste = withView((view) => {
    void readClipboardText().then(
      (text) => {
        if (text) view.dispatch(view.state.replaceSelection(text), { userEvent: "input.paste" });
      },
      () => undefined,
    );
  });
  const selectAll = withView((view) =>
    view.dispatch({ selection: EditorSelection.single(0, view.state.doc.length) }),
  );
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent
        className="w-64"
        // Back to the editor the menu was opened on.
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          getEditorView(editorKey)?.focus();
        }}
      >
        <ContextMenuItem onSelect={handlers.runCurrent}>
          Run Current
          <Shortcut action="query.run-current" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlers.run}>
          Run All
          <Shortcut action="query.run" />
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={cut}>
          Cut
          <Shortcut combo="Mod+X" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={copy}>
          Copy
          <Shortcut combo="Mod+C" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={paste}>
          Paste
          <Shortcut combo="Mod+V" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={selectAll}>
          Select All
          <Shortcut combo="Mod+A" />
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={withView((v) => void toggleLineComment(v))}>
          Toggle Line Comment
          <Shortcut action="query.comment-line" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={withView((v) => void toggleBlockCommentSmart(v))}>
          Toggle Block Comment
          <Shortcut action="query.comment-block" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlers.format}>
          Beautify SQL
          <Shortcut action="query.format" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlers.minify}>
          Uglify SQL
          <Shortcut action="query.minify" />
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handlers.addToQueries}>
          Add to Queries…
          <Shortcut action="query.add-to-queries" />
        </ContextMenuItem>
        <ContextMenuItem onSelect={handlers.saveSelection}>
          Save Selected SQL Statements As…
          <Shortcut action="query.save-selection" />
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={handlers.split}>
          {split ? "Close Split Pane" : "Split Pane Horizontally"}
          <Shortcut action="query.split" />
        </ContextMenuItem>
        <EditorMenuExtensions tabId={tabId} />
      </ContextMenuContent>
    </ContextMenu>
  );
}

interface PanesProps {
  tabId: string;
  connectionId: string;
  /** A core dialect, or a lane's language. */
  dialect: EditorLanguage;
  currentDatabase: string | null;
  handlers: EditorPaneHandlers;
}

function Pane({
  pane,
  active,
  split,
  ...props
}: PanesProps & { pane: EditorPane; active: boolean; split: boolean }) {
  const { tabId, handlers } = props;
  const editorKey = pane === "split" ? splitKey(tabId) : tabId;
  // A closed split pane forgets its document. This runs after the editor's own cleanup, which
  // stashes its state (it is kept while the tab is only hidden).
  useEffect(() => {
    if (pane !== "split") return;
    return () => {
      if (!isSplitOpen(tabId)) dropEditor(editorKey);
    };
  }, [pane, tabId, editorKey]);
  return (
    <EditorContextMenu tabId={tabId} editorKey={editorKey} split={split} handlers={handlers}>
      <div
        className="relative h-full min-h-0"
        data-pane={pane}
        data-active-pane={split && active ? "" : undefined}
        // The bar's commands act on the pane used last (focus or right-click).
        onFocusCapture={() => setFocusedPane(tabId, pane)}
        onContextMenuCapture={() => setFocusedPane(tabId, pane)}
      >
        {split && (
          <div
            aria-hidden
            className={cn(
              "pointer-events-none absolute inset-y-0 left-0 z-[3] w-0.5",
              active ? "bg-brand/60" : "bg-transparent",
            )}
          />
        )}
        <SqlEditor
          tabId={editorKey}
          connectionId={props.connectionId}
          dialect={props.dialect}
          currentDatabase={props.currentDatabase}
        />
        {pane === "split" && (
          <button
            type="button"
            aria-label="Close split pane"
            title="Close split pane"
            onClick={handlers.split}
            className="text-muted-foreground hover:text-foreground hover:bg-muted absolute top-1 right-1.5 z-[3] flex size-5 items-center justify-center rounded"
          >
            <X className="size-3" />
          </button>
        )}
      </div>
    </EditorContextMenu>
  );
}

/**
 * The tab's SQL editor, split in two panes stacked top / bottom while the split is on (⇧⌘D). The
 * first pane keeps its place in the tree, so opening or closing the split never remounts it.
 */
export function EditorPanes(props: PanesProps) {
  const split = useSplitOpen(props.tabId);
  const activeKey = useActiveEditorKey(props.tabId);
  const activeSplit = activeKey !== props.tabId;
  return (
    <ResizablePanelGroup orientation="vertical" className="h-full min-h-0">
      <ResizablePanel id="pane-main" minSize={48}>
        <Pane {...props} pane="main" active={!activeSplit} split={split} />
      </ResizablePanel>
      {split && (
        <>
          <ResizableHandle />
          <ResizablePanel id="pane-split" minSize={48}>
            <Pane {...props} pane="split" active={activeSplit} split />
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  );
}
