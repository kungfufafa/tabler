import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import {
  canRedo,
  deleteRows,
  emptyStaging,
  insertRow,
  insertRows,
  pendingCount,
  revertCell,
  revertRow,
  setCell,
  setCells,
  setRowError,
  redo,
  undo,
} from "./model";

const orig: Value[] = [{ t: "int", v: 1 }, { t: "text", v: "a" }, { t: "null" }];

describe("staging model", () => {
  it("stages, reverts to original, and undoes", () => {
    let s = emptyStaging();
    s = setCell(s, "pk:1", 1, { t: "text", v: "b" }, orig);
    expect(pendingCount(s)).toBe(1);
    expect(s.changes.get("pk:1")?.status).toBe("updated");
    s = setCell(s, "pk:1", 1, { t: "text", v: "a" }, orig);
    expect(pendingCount(s)).toBe(0);
    s = setCell(s, "pk:1", 2, { t: "text", v: "x" }, orig);
    s = setCell(s, "pk:1", 1, { t: "text", v: "y" }, orig);
    expect(s.changes.get("pk:1")?.patch.size).toBe(2);
    s = revertCell(s, "pk:1", 1);
    expect(s.changes.get("pk:1")?.patch.size).toBe(1);
    s = undo(s);
    expect(s.changes.get("pk:1")?.patch.size).toBe(2);
    s = revertRow(s, "pk:1");
    expect(pendingCount(s)).toBe(0);
  });

  it("inserts, edits and deletes new rows", () => {
    let s = insertRow(emptyStaging(), "new:1", [
      { t: "default" },
      { t: "text", v: "" },
      { t: "null" },
    ]);
    expect(s.insertOrder).toEqual(["new:1"]);
    s = setCell(s, "new:1", 1, { t: "text", v: "n" }, undefined);
    expect(s.changes.get("new:1")?.patch.get(1)).toEqual({ t: "text", v: "n" });
    s = deleteRows(s, [{ rowKey: "new:1", original: undefined }]);
    expect(s.insertOrder).toEqual([]);
    expect(pendingCount(s)).toBe(0);
    s = undo(s);
    expect(s.insertOrder).toEqual(["new:1"]);
  });

  it("marks existing rows deleted and ignores edits on them", () => {
    let s = deleteRows(emptyStaging(), [{ rowKey: "pk:1", original: orig }]);
    expect(s.changes.get("pk:1")?.status).toBe("deleted");
    s = setCell(s, "pk:1", 1, { t: "text", v: "z" }, orig);
    expect(s.changes.get("pk:1")?.patch.size).toBe(0);
    s = revertRow(s, "pk:1");
    expect(pendingCount(s)).toBe(0);
  });

  it("setCells stages a batch as one undo step and undo restores every row", () => {
    const orig2: Value[] = [{ t: "int", v: 2 }, { t: "text", v: "b" }, { t: "null" }];
    let s = setCell(emptyStaging(), "pk:2", 2, { t: "text", v: "kept" }, orig2);
    s = insertRow(s, "new:1", [{ t: "default" }, { t: "text", v: "" }, { t: "null" }]);
    const before = s;
    s = setCells(s, [
      { rowKey: "pk:1", col: 1, value: { t: "text", v: "x" }, original: orig },
      { rowKey: "pk:1", col: 2, value: { t: "text", v: "y" }, original: orig },
      { rowKey: "pk:2", col: 1, value: { t: "null" }, original: orig2 },
      { rowKey: "new:1", col: 1, value: { t: "text", v: "n" }, original: undefined },
    ]);
    expect(s.undo.length).toBe(before.undo.length + 1);
    expect(s.undo.at(-1)?.rows.map((r) => r.rowKey)).toEqual(["pk:1", "pk:2", "new:1"]);
    expect(s.changes.get("pk:1")?.patch.size).toBe(2);
    expect(s.changes.get("pk:2")?.patch.get(1)).toEqual({ t: "null" });
    expect(s.changes.get("pk:2")?.patch.get(2)).toEqual({ t: "text", v: "kept" });
    expect(s.changes.get("new:1")?.patch.get(1)).toEqual({ t: "text", v: "n" });
    s = undo(s);
    expect(s.changes).toEqual(before.changes);
    expect(s.insertOrder).toEqual(["new:1"]);
    expect(s.undo.length).toBe(before.undo.length);
  });

  it("setCells skips deleted rows and no-op batches", () => {
    let s = deleteRows(emptyStaging(), [{ rowKey: "pk:1", original: orig }]);
    const before = s;
    s = setCells(s, [{ rowKey: "pk:1", col: 1, value: { t: "text", v: "z" }, original: orig }]);
    expect(s).toBe(before);
    s = setCells(s, [{ rowKey: "pk:9", col: 1, value: { t: "text", v: "a" }, original: orig }]);
    expect(s).toBe(before);
  });

  it("deleteRows is one undo step", () => {
    let s = insertRow(emptyStaging(), "new:1", []);
    s = deleteRows(s, [
      { rowKey: "pk:1", original: orig },
      { rowKey: "pk:2", original: orig },
      { rowKey: "new:1", original: undefined },
    ]);
    expect(pendingCount(s)).toBe(2);
    expect(s.insertOrder).toEqual([]);
    s = undo(s);
    expect(pendingCount(s)).toBe(1);
    expect(s.insertOrder).toEqual(["new:1"]);
  });

  it("redoes undone steps in order, and undoes the redo", () => {
    let s = setCell(emptyStaging(), "pk:1", 1, { t: "text", v: "b" }, orig);
    const one = s;
    s = setCells(s, [
      { rowKey: "pk:1", col: 2, value: { t: "text", v: "x" }, original: orig },
      { rowKey: "pk:2", col: 1, value: { t: "null" }, original: orig },
    ]);
    const two = s;
    expect(canRedo(s)).toBe(false);
    s = undo(undo(s));
    expect(pendingCount(s)).toBe(0);
    expect(s.redo).toHaveLength(2);
    expect(canRedo(s)).toBe(true);
    s = redo(s);
    expect(s.changes).toEqual(one.changes);
    s = redo(s);
    expect(s.changes).toEqual(two.changes);
    expect(s.undo).toHaveLength(2);
    expect(canRedo(s)).toBe(false);
    expect(redo(s)).toBe(s);
    // A redone step is an ordinary undo step again.
    s = undo(s);
    expect(s.changes).toEqual(one.changes);
    expect(s.version).toBeGreaterThan(two.version);
  });

  it("redoes inserts and deletes with their row order", () => {
    let s = insertRow(emptyStaging(), "new:1", [{ t: "default" }]);
    s = insertRow(s, "new:2", [{ t: "default" }]);
    s = deleteRows(s, [
      { rowKey: "new:1", original: undefined },
      { rowKey: "pk:1", original: orig },
    ]);
    const deleted = s;
    expect(s.insertOrder).toEqual(["new:2"]);
    s = undo(s);
    expect(s.insertOrder).toEqual(["new:1", "new:2"]);
    expect(s.changes.has("pk:1")).toBe(false);
    s = redo(s);
    expect(s.insertOrder).toEqual(["new:2"]);
    expect(s.changes).toEqual(deleted.changes);
    s = undo(undo(undo(s)));
    expect(s.insertOrder).toEqual([]);
    expect(pendingCount(s)).toBe(0);
    s = redo(redo(s));
    expect(s.insertOrder).toEqual(["new:1", "new:2"]);
  });

  it("a new edit clears the redo stack; no-op edits and apply errors keep it", () => {
    let s = setCell(emptyStaging(), "pk:1", 1, { t: "text", v: "b" }, orig);
    s = setCell(s, "pk:1", 2, { t: "text", v: "c" }, orig);
    s = undo(s);
    expect(canRedo(s)).toBe(true);
    // Nothing applies (no original snapshot): not an edit.
    expect(
      setCells(s, [{ rowKey: "pk:9", col: 1, value: { t: "null" }, original: undefined }]),
    ).toBe(s);
    s = setRowError(s, "pk:1", "boom");
    expect(canRedo(s)).toBe(true);
    s = setCell(s, "pk:1", 1, { t: "text", v: "z" }, orig);
    expect(canRedo(s)).toBe(false);
    expect(redo(s)).toBe(s);
    expect(insertRow(undo(s), "new:1", []).redo).toEqual([]);
    expect(deleteRows(undo(s), [{ rowKey: "pk:2", original: orig }]).redo).toEqual([]);
    expect(revertRow(undo(s), "pk:1").redo).toEqual([]);
  });
});

describe("insertRows", () => {
  it("adds several rows in order as one undo step", () => {
    const t = (v: string): Value => ({ t: "text", v });
    const s = insertRows(emptyStaging(), [
      { rowKey: "new:a", values: [t("a")] },
      { rowKey: "new:b", values: [t("b")] },
    ]);
    expect(s.insertOrder).toEqual(["new:a", "new:b"]);
    expect(s.changes.get("new:b")).toMatchObject({ status: "inserted" });
    expect(pendingCount(s)).toBe(2);
    const back = undo(s);
    expect(pendingCount(back)).toBe(0);
    expect(back.insertOrder).toEqual([]);
    expect(insertRows(s, [])).toBe(s);
  });
});
