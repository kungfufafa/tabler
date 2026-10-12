import type { Completion, CompletionSource } from "@codemirror/autocomplete";
import { EditorState, Facet, type Extension } from "@codemirror/state";
import type { SavedQuery } from "@/lib/ipc";
import { queriesFor, useSavedQueriesStore } from "./store";

/**
 * Keyword binding (TablePlus): typing a saved query's keyword in the SQL editor offers the query
 * as a completion (label = keyword, detail = name, boosted to the top); accepting it replaces the
 * keyword with the saved SQL.
 */

/** The connection profile of the editor (which saved queries it sees); null: only global ones. */
export const savedQueryProfile = Facet.define<string | null, string | null>({
  combine: (values) => values[0] ?? null,
});

/** What a keyword can be made of while it is typed (a keyword is one word, see the backend). */
const KEYWORD_BEFORE = /[^\s()[\]{},;'"`]+/;
const KEYWORD_VALID = /^[^\s()[\]{},;'"`]*$/;
const INFO_MAX = 600;

export function keywordCompletions(
  queries: readonly SavedQuery[],
  profileId: string | null,
): Completion[] {
  return queriesFor(queries, profileId)
    .filter((q) => q.keyword)
    .map((q) => ({
      label: q.keyword!,
      detail: q.name,
      type: "keyword-binding",
      boost: 99,
      apply: q.sql,
      info: q.sql.length > INFO_MAX ? `${q.sql.slice(0, INFO_MAX)}…` : q.sql,
    }));
}

export const savedQueryCompletion: CompletionSource = (ctx) => {
  const word = ctx.matchBefore(KEYWORD_BEFORE);
  if (!word && !ctx.explicit) return null;
  const options = keywordCompletions(
    useSavedQueriesStore.getState().queries,
    ctx.state.facet(savedQueryProfile),
  );
  if (options.length === 0) return null;
  return { from: word?.from ?? ctx.pos, options, validFor: KEYWORD_VALID };
};

/** The completion source (at every position) plus the editor's profile. */
export function savedQueryCompletionExtension(profileId: string | null): Extension {
  return [
    savedQueryProfile.of(profileId),
    EditorState.languageData.of(() => [{ autocomplete: savedQueryCompletion }]),
  ];
}
