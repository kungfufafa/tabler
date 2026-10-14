import { describe, expect, it } from "vitest";
import {
  DEFAULT_TIMEOUT,
  defaultObject,
  duplicateObject,
  formatOptions,
  inputValue,
  newObject,
  normalizeObjects,
  parseOptions,
  removeObject,
  unwire,
  wire,
  type BoardObject,
} from "./model";

const obj = (kind: BoardObject["kind"], id: string, patch: Partial<BoardObject> = {}) => ({
  ...defaultObject(kind, id, id),
  ...patch,
});

describe("board model", () => {
  it("round-trips through JSON (what the app database and board files store)", () => {
    const objects = [
      obj("bar", "a", {
        sql: "SELECT 1",
        refresh: { mode: "interval", seconds: 30 },
        x: "month",
        y: ["total"],
        colors: [3],
      }),
      obj("input", "b", { variable: "q", targets: ["a"], inputType: "number", defaultValue: "5" }),
      obj("dropdown", "c", {
        variable: "role",
        options: [{ value: "admin", label: "Admin" }],
        optionsSource: "static",
      }),
    ];
    expect(normalizeObjects(JSON.parse(JSON.stringify(objects)))).toEqual(objects);
  });

  it("fills defaults and drops what it cannot read", () => {
    const out = normalizeObjects([
      { id: "a", kind: "scoreboard" },
      { id: "b", kind: "sparkline" },
      { kind: "bar" },
      "nope",
      { id: "a", kind: "table" },
      {
        id: "c",
        kind: "input",
        variable: "1bad",
        layout: { x: 20, y: -3, w: 99, h: 0 },
        refresh: { mode: "interval", seconds: 0 },
        targets: ["a", "c", "zzz", "a"],
      },
    ]);
    expect(out.map((o) => o.id)).toEqual(["a", "c"]);
    expect(out[0]).toEqual(defaultObject("scoreboard", "a", "Scoreboard"));
    const c = out[1]!;
    expect(c.variable).toBe("");
    expect(c.layout).toEqual({ x: 0, y: 0, w: 12, h: 2 });
    expect(c.refresh).toEqual({ mode: "interval", seconds: 1 });
    // Wires only to other objects that exist, once each.
    expect(c.targets).toEqual(["a"]);
    expect(normalizeObjects({ not: "a list" })).toEqual([]);
  });

  it("gives an object the default query timeout, and keeps a stored one within 5 to 600 s", () => {
    const out = normalizeObjects([
      { id: "old", kind: "table", sql: "SELECT 1" },
      { id: "low", kind: "table", timeoutSecs: 1 },
      { id: "high", kind: "table", timeoutSecs: 99_999 },
      { id: "ok", kind: "table", timeoutSecs: 90.4 },
      { id: "junk", kind: "table", timeoutSecs: "30" },
    ]);
    expect(out.map((o) => o.timeoutSecs)).toEqual([60, 5, 600, 90, 60]);
    expect(defaultObject("table", "a", "A").timeoutSecs).toBe(DEFAULT_TIMEOUT);
    expect(DEFAULT_TIMEOUT).toBe(60);
    // It is part of the stored object.
    const stored = JSON.parse(JSON.stringify([obj("table", "t", { timeoutSecs: 120 })]));
    expect(normalizeObjects(stored)[0]!.timeoutSecs).toBe(120);
  });

  it("adds objects on the first free spot with unique titles and variables", () => {
    const a = newObject("bar", [], "a");
    expect(a.layout).toEqual({ x: 0, y: 0, w: 6, h: 6 });
    expect(a.title).toBe("Bar chart 1");
    const b = newObject("bar", [a], "b");
    expect(b.layout).toEqual({ x: 6, y: 0, w: 6, h: 6 });
    expect(b.title).toBe("Bar chart 2");
    const c = newObject("input", [a, b], "c");
    expect(c.layout).toEqual({ x: 0, y: 6, w: 3, h: 2 });
    expect(c.variable).toBe("input1");
    expect(newObject("input", [a, b, c], "d").variable).toBe("input2");
    expect(newObject("button", [], "e").title).toBe("Refresh 1");
  });

  it("duplicates without the wires", () => {
    const src = obj("input", "a", { title: "Search 1", variable: "q1", targets: ["x"] });
    const copy = duplicateObject(src, [src], "b");
    expect(copy).toMatchObject({ id: "b", title: "Search 2", variable: "q2", targets: [] });
    // Beside the original, not on top of it.
    expect(copy.layout).toEqual({ ...src.layout, x: src.layout.w });
  });

  it("wires a source to an object and switches the target to Refresh on event", () => {
    const list = [
      obj("input", "in", { variable: "q" }),
      obj("table", "t", { refresh: { mode: "interval", seconds: 10 } }),
      obj("button", "btn"),
    ];
    const wired = wire(list, "in", "t");
    expect(wired[0]!.targets).toEqual(["t"]);
    expect(wired[1]!.refresh).toEqual({ mode: "event" });
    // Once only; never to itself, from a non-source, or to a button.
    expect(wire(wired, "in", "t")[0]!.targets).toEqual(["t"]);
    expect(wire(list, "in", "in")).toEqual(list);
    expect(wire(list, "t", "in")).toEqual(list);
    expect(wire(list, "in", "btn")).toEqual(list);
    expect(unwire(wired, "in", "t")[0]!.targets).toEqual([]);
    // Deleting a target drops the wires to it.
    expect(removeObject(wired, "t")).toEqual([{ ...wired[0]!, targets: [] }, wired[2]]);
  });

  it("types input values", () => {
    expect(inputValue("number", " 42 ")).toEqual({ t: "int", v: 42 });
    expect(inputValue("number", "-1.50")).toEqual({ t: "decimal", v: "-1.50" });
    expect(inputValue("number", "9007199254740993")).toEqual({
      t: "decimal",
      v: "9007199254740993",
    });
    expect(inputValue("number", "abc")).toEqual({ t: "null" });
    expect(inputValue("number", "")).toEqual({ t: "null" });
    expect(inputValue("date", "2026-10-13")).toEqual({ t: "date", v: "2026-10-13" });
    expect(inputValue("date", "")).toEqual({ t: "null" });
    expect(inputValue("text", "")).toEqual({ t: "text", v: "" });
  });

  it("reads and writes static dropdown options", () => {
    const options = parseOptions("admin | Administrator\n\n  viewer \nx |");
    expect(options).toEqual([
      { value: "admin", label: "Administrator" },
      { value: "viewer", label: "viewer" },
      { value: "x", label: "x" },
    ]);
    expect(parseOptions(formatOptions(options))).toEqual(options);
  });
});
