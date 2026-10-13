import { describe, expect, it } from "vitest";
import { directiveAbove, parseDirective } from "./directive";

describe("chart directive", () => {
  it("parses the three charts with the TablePlus or Tabler keyword", () => {
    expect(parseDirective("-- TablePlus BarChart x: col_a, y: col_b")).toEqual({
      kind: "bar",
      x: "col_a",
      y: ["col_b"],
    });
    expect(parseDirective("-- Tabler LineChart x: day, y: views")).toEqual({
      kind: "line",
      x: "day",
      y: ["views"],
    });
    expect(parseDirective("--tableplus piechart x:status,y:total")).toEqual({
      kind: "pie",
      x: "status",
      y: ["total"],
    });
    expect(parseDirective("# TablePlus BarChart x: a, y: b")?.kind).toBe("bar");
    expect(parseDirective("/* Tabler PieChart x: a, y: b */")).toEqual({
      kind: "pie",
      x: "a",
      y: ["b"],
    });
  });

  it("takes several y columns, in any order, quoted or not", () => {
    expect(parseDirective('-- TablePlus BarChart y: revenue, "Cost $", `tax` x: month')).toEqual({
      kind: "bar",
      x: "month",
      y: ["revenue", "Cost $", "tax"],
    });
    expect(parseDirective("-- TablePlus LineChart x: d, y: a, b, c,")).toEqual({
      kind: "line",
      x: "d",
      y: ["a", "b", "c"],
    });
    // Without arguments: the first column against every numeric one.
    expect(parseDirective("-- TablePlus BarChart")).toEqual({ kind: "bar", x: null, y: [] });
  });

  it("ignores other comments", () => {
    expect(parseDirective("-- just a note about the chart")).toBeNull();
    expect(parseDirective("-- TablePlus ScatterChart x: a, y: b")).toBeNull();
    expect(parseDirective("-- TablePlusBarChart x: a")).toBeNull();
    expect(parseDirective("select 1 -- TablePlus BarChart x: a, y: b")).toBeNull();
  });

  it("finds the directive right above a statement", () => {
    const text = [
      "select 1;",
      "-- TablePlus BarChart x: name, y: total",
      "-- totals per customer",
      "",
      "select name, total from t;",
      "select 2;",
    ].join("\n");
    const at = (s: string) => text.indexOf(s);
    expect(directiveAbove(text, at("select name"))).toEqual({
      kind: "bar",
      x: "name",
      y: ["total"],
    });
    // The statement after it has code right above: no directive.
    expect(directiveAbove(text, at("select 2"))).toBeNull();
    expect(directiveAbove(text, at("select 1"))).toBeNull();
    // A directive before an earlier statement does not carry over.
    const two = "-- Tabler PieChart x: a, y: b\nselect a, b from t;\nselect c from u";
    expect(directiveAbove(two, two.indexOf("select c"))).toBeNull();
    // Inline block comment on the statement's own line.
    const inline = "/* Tabler LineChart x: d, y: v */ select d, v from t";
    expect(directiveAbove(inline, inline.indexOf("select"))?.kind).toBe("line");
  });
});
