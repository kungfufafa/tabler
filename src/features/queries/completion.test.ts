import { CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { beforeEach, describe, expect, it } from "vitest";
import type { SavedQuery } from "@/lib/ipc";
import { languageExtension } from "@/features/editor/cmExtensions";
import {
  keywordCompletions,
  savedQueryCompletion,
  savedQueryCompletionExtension,
} from "./completion";
import { resetSavedQueriesStore, useSavedQueriesStore } from "./store";

function query(id: string, extra: Partial<SavedQuery>): SavedQuery {
  return {
    id,
    name: id,
    keyword: null,
    sql: `select '${id}'`,
    folderId: null,
    profileId: null,
    createdAt: "2026-01-01T00:00:00.000Z",
    updatedAt: "2026-01-01T00:00:00.000Z",
    ...extra,
  };
}

const QUERIES = [
  query("recent", { name: "Recent users", keyword: "recent", sql: "select * from users;" }),
  query("mine", { name: "Mine", keyword: "rmine", profileId: "p1", sql: "select 1" }),
  query("theirs", { name: "Theirs", keyword: "rtheirs", profileId: "p2" }),
  query("nokw", { name: "No keyword" }),
];

function complete(doc: string, profileId: string | null, explicit = false) {
  const state = EditorState.create({ doc, extensions: savedQueryCompletionExtension(profileId) });
  return savedQueryCompletion(
    new CompletionContext(state, doc.length, explicit),
  ) as CompletionResult | null;
}

beforeEach(() => {
  resetSavedQueriesStore();
  useSavedQueriesStore.setState({ queries: QUERIES, loaded: true });
});

describe("keyword binding completion", () => {
  it("offers the keywords the connection sees, labelled with the keyword, detailed with the name", () => {
    const options = keywordCompletions(QUERIES, "p1");
    expect(options.map((o) => [o.label, o.detail, o.boost])).toEqual([
      ["recent", "Recent users", 99],
      ["rmine", "Mine", 99],
    ]);
    // Accepting replaces the keyword with the saved SQL.
    expect(options[0]!.apply).toBe("select * from users;");
    expect(keywordCompletions(QUERIES, null).map((o) => o.label)).toEqual(["recent"]);
  });

  it("completes the word before the cursor", () => {
    const result = complete("select 1;\nrec", "p1");
    expect(result?.from).toBe(10);
    expect(result?.options.map((o) => o.label)).toEqual(["recent", "rmine"]);
    expect(result?.validFor instanceof RegExp && result.validFor.test("recen")).toBe(true);
  });

  it("stays quiet without a word unless asked (⌃Space), and without keywords", () => {
    expect(complete("select ", "p1")).toBeNull();
    expect(complete("select ", "p1", true)?.from).toBe(7);
    useSavedQueriesStore.setState({ queries: [QUERIES[3]!] });
    expect(complete("rec", "p1")).toBeNull();
  });

  it("is part of the editor's language, next to the SQL completions", () => {
    const state = EditorState.create({
      doc: "rec",
      extensions: languageExtension({ dialect: "postgres", profileId: "p2" }),
    });
    const sources = state.languageDataAt<unknown>("autocomplete", 3);
    expect(sources).toContain(savedQueryCompletion);
    const result = savedQueryCompletion(new CompletionContext(state, 3, false)) as CompletionResult;
    expect(result.options.map((o) => o.label)).toEqual(["recent", "rtheirs"]);
  });
});
