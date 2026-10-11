import { ChangeSet } from "@codemirror/state";
import { afterEach, describe, expect, it } from "vitest";
import { anchorSubmission, dropAnchors, recordDocChanges, toCurrentDoc } from "./docAnchors";

const TAB = "t-anchors";
const insert = (docLength: number, at: number, text: string) =>
  ChangeSet.of({ from: at, insert: text }, docLength).desc;

afterEach(() => dropAnchors(TAB));

describe("docAnchors", () => {
  // "select pg_sleep(0.3);\nfoo bar;" — "foo" is at 22..25.
  const doc = "select pg_sleep(0.3);\nfoo bar;";
  const foo = { from: 22, to: 25 };

  it("returns ranges unchanged while the document is untouched", () => {
    anchorSubmission(TAB, "run", "r1");
    expect(toCurrentDoc(TAB, "r1", foo)).toEqual(foo);
  });

  it("maps ranges through edits made after the submission", () => {
    anchorSubmission(TAB, "run", "r1");
    recordDocChanges(TAB, insert(doc.length, 0, "-- note\n"));
    expect(toCurrentDoc(TAB, "r1", foo)).toEqual({ from: 30, to: 33 });
    // Edits keep composing.
    recordDocChanges(TAB, insert(doc.length + 8, 0, "\n"));
    expect(toCurrentDoc(TAB, "r1", foo)).toEqual({ from: 31, to: 34 });
  });

  it("drops a range whose text was deleted, or edited when asked to", () => {
    anchorSubmission(TAB, "run", "r1");
    // Type inside "foo" → "fXoo".
    recordDocChanges(TAB, insert(doc.length, 23, "X"));
    expect(toCurrentDoc(TAB, "r1", foo)).toEqual({ from: 22, to: 26 });
    expect(toCurrentDoc(TAB, "r1", foo, { unlessEdited: true })).toBeNull();
    // Typing right before it does not count as editing it.
    anchorSubmission(TAB, "run", "r2");
    recordDocChanges(TAB, insert(doc.length, 22, "  "));
    expect(toCurrentDoc(TAB, "r2", foo, { unlessEdited: true })).toEqual({ from: 24, to: 27 });
    // Deleting the whole token.
    anchorSubmission(TAB, "run", "r3");
    recordDocChanges(TAB, ChangeSet.of({ from: 21, to: 29 }, doc.length).desc);
    expect(toCurrentDoc(TAB, "r3", foo)).toBeNull();
  });

  it("tracks the latest run and explain separately and forgets replaced ones", () => {
    anchorSubmission(TAB, "run", "r1");
    anchorSubmission(TAB, "plan", "p1");
    recordDocChanges(TAB, insert(doc.length, 0, "ab"));
    anchorSubmission(TAB, "plan", "p2");
    expect(toCurrentDoc(TAB, "r1", foo)).toEqual({ from: 24, to: 27 });
    expect(toCurrentDoc(TAB, "p2", foo)).toEqual(foo);
    expect(toCurrentDoc(TAB, "p1", foo)).toBeNull();
    expect(toCurrentDoc("other-tab", "r1", foo)).toBeNull();
  });

  it("clamps ranges past the end of the submitted document", () => {
    anchorSubmission(TAB, "run", "r1");
    recordDocChanges(TAB, insert(5, 0, "x"));
    expect(toCurrentDoc(TAB, "r1", { from: 3, to: 50 })).toEqual({ from: 4, to: 6 });
  });
});
