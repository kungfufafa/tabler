import { describe, expect, it } from "vitest";
import type { Value } from "@/lib/ipc";
import {
  deleteRows,
  emptyStaging,
  insertRow,
  pendingCount,
  revertCell,
  revertRow,
  setCell,
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
});
