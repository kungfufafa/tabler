import { EditorView } from "@codemirror/view";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { ensureSavedQueries } from "@/features/queries/store";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { usePrefsStore } from "@/stores/prefs";
import {
  editorPrefsOf,
  languageConf,
  languageExtension,
  prefsConf,
  prefsExtension,
  type LanguageConfig,
} from "./cmExtensions";
import { completionDefaultSchema, completionNamespace } from "./completion";
import { registerView, stashEditorState, takeEditorState } from "./editorStates";
import type { SqlDialect } from "./scanner";

export function SqlEditor({
  tabId,
  connectionId,
  dialect,
  currentDatabase,
  autoFocus = true,
}: {
  tabId: string;
  connectionId: string;
  dialect: SqlDialect;
  currentDatabase: string | null;
  autoFocus?: boolean;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);

  const completion = useQuery({
    queryKey: schemaKeys.completion(connectionId),
    queryFn: () => ipc.schema.completion(connectionId),
    staleTime: 5 * 60_000,
  });
  const upperCaseKeywords = usePrefsStore((s) => s.prefs.editorUppercaseKeywords);
  const prefs = usePrefsStore(useShallow((s) => editorPrefsOf(s.prefs)));
  const profileId = useConnectionsStore((s) => s.byId[connectionId]?.info.profileId ?? null);

  // Keyword binding completes from the saved queries.
  useEffect(() => void ensureSavedQueries(), []);

  const language = useMemo<LanguageConfig>(
    () => ({
      dialect,
      schema: completionNamespace(completion.data),
      defaultSchema: completionDefaultSchema(dialect, completion.data, currentDatabase),
      upperCaseKeywords,
      profileId,
    }),
    [dialect, completion.data, currentDatabase, upperCaseKeywords, profileId],
  );
  const languageRef = useRef(language);
  languageRef.current = language;

  // One EditorView per mount; the EditorState (doc, undo history, selection) outlives it.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const entry = takeEditorState(tabId, languageRef.current);
    const view = new EditorView({
      state: entry.state,
      parent: host,
      scrollTo: entry.scroll ?? undefined,
    });
    viewRef.current = view;
    const unregister = registerView(tabId, view);
    if (autoFocus) view.focus();
    return () => {
      stashEditorState(tabId, view.state, view.scrollSnapshot());
      unregister();
      viewRef.current = null;
      view.destroy();
    };
    // autoFocus only matters when the view is created.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: languageConf.reconfigure(languageExtension(language)) });
  }, [language]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: prefsConf.reconfigure(prefsExtension(prefs)) });
  }, [prefs]);

  return <div ref={hostRef} className="h-full min-h-0 overflow-hidden" data-testid="sql-editor" />;
}
