import { EditorView } from "@codemirror/view";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { ensureSavedQueries } from "@/features/queries/store";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, type TableRef } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { usePrefsStore } from "@/stores/prefs";
import {
  editorPrefsOf,
  languageConf,
  languagePrefsOf,
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
  const completionPrefs = usePrefsStore(useShallow((s) => languagePrefsOf(s.prefs)));
  const prefs = usePrefsStore(useShallow((s) => editorPrefsOf(s.prefs)));
  const defaultSchema = completionDefaultSchema(dialect, completion.data, currentDatabase);
  // Function completion: the default schema's routines (SQLite has none).
  const routines = useQuery({
    queryKey: ["completion-routines", connectionId, defaultSchema ?? ""],
    queryFn: () => ipc.objects.routines(connectionId, defaultSchema!),
    enabled: dialect !== "sqlite" && !!defaultSchema,
    staleTime: 5 * 60_000,
    select: (list) => list.map((r) => ({ name: r.name, kind: r.kind })),
  });
  // JOIN … ON suggestions read the tables' foreign keys (cached like the sidebar's metadata).
  const qc = useQueryClient();
  const tableMeta = useCallback(
    (table: TableRef) =>
      qc.fetchQuery({
        queryKey: schemaKeys.table(connectionId, table.schema, table.name),
        queryFn: () => ipc.schema.table(connectionId, table),
        staleTime: 5 * 60_000,
      }),
    [qc, connectionId],
  );
  const profileId = useConnectionsStore((s) => s.byId[connectionId]?.info.profileId ?? null);

  // Keyword binding completes from the saved queries.
  useEffect(() => void ensureSavedQueries(), []);

  const language = useMemo<LanguageConfig>(
    () => ({
      dialect,
      schema: completionNamespace(completion.data),
      defaultSchema,
      tables: completion.data?.schemas as LanguageConfig["tables"],
      routines: routines.data,
      tableMeta,
      profileId,
      ...completionPrefs,
    }),
    [dialect, completion.data, defaultSchema, routines.data, tableMeta, profileId, completionPrefs],
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
