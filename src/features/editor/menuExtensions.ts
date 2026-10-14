import { createElement, Fragment, type ReactNode } from "react";

/**
 * Extension point of the SQL editor's context menu (Plugins ▸ …), next to the grid's
 * (`datagrid/menuExtensions.ts`) and the sidebar's (`objects/menu.ts`). The menu hands the
 * extension the query tab it was opened in; the editor's text and selection are read when a
 * command runs.
 */
export interface EditorMenuContext {
  /** The query tab the right-clicked pane belongs to. */
  tabId: string;
}

export type EditorMenuExtension = (context: EditorMenuContext) => ReactNode;

const extensions = new Set<EditorMenuExtension>();

/** Adds entries at the end of every editor pane's context menu. Returns the remover. */
export function registerEditorMenuExtension(extension: EditorMenuExtension): () => void {
  extensions.add(extension);
  return () => void extensions.delete(extension);
}

/** What the extensions add to an editor's context menu (rendered while it is open). */
export function renderEditorMenuExtensions(context: EditorMenuContext): ReactNode {
  return [...extensions].map((ext, i) => createElement(Fragment, { key: i }, ext(context)));
}

/**
 * The extensions' entries as a component: it mounts with the open menu (Radix renders the content
 * only while it is open), so every opening reads the current state, e.g. the enabled plugins.
 */
export function EditorMenuExtensions({ tabId }: EditorMenuContext): ReactNode {
  return renderEditorMenuExtensions({ tabId });
}
