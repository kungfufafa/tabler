import { describe, expect, it } from "vitest";
import type { ResultColumn, Value } from "@/lib/ipc";
import { buildChartData, sampleIndexes, toNumber, type ChartData } from "./data";
import { parseDirective } from "./directive";
import { chartFileName } from "./export";
import { formatPercent, formatTick, formatValue, niceTicks } from "./scale";
import { barPath, clip, layoutChart, nearestIndex } from "./scene";
import { sceneToSvg, xmlEscape } from "./svg";

const col = (name: string, kind: ResultColumn["kind"]): ResultColumn => ({
  name,
  dataType: kind,
  kind,
  nullable: null,
  table: null,
});
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });

const COLUMNS = [col("month", "text"), col("revenue", "int"), col("cost", "decimal")];
const ROWS: Value[][] = [
  [text("Jan"), int(120), { t: "decimal", v: "80.5" }],
  [text("Feb"), int(90), { t: "decimal", v: "95" }],
  [text("Mar"), { t: "null" }, { t: "decimal", v: "60" }],
  [text("Apr"), int(150), { t: "decimal", v: "70.25" }],
];

function data(directive: string, columns = COLUMNS, rows = ROWS): ChartData {
  const d = parseDirective(directive);
  if (!d) throw new Error("bad directive");
  const built = buildChartData(d, columns, rows);
  if (!built.ok) throw new Error(built.error);
  return built.data;
}

describe("chart scales", () => {
  it("picks round ticks", () => {
    expect(niceTicks(0, 150).ticks).toEqual([0, 50, 100, 150]);
    expect(niceTicks(-12, 37).ticks).toEqual([-20, -10, 0, 10, 20, 30, 40]);
    expect(niceTicks(0.1, 0.9).ticks).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1]);
    expect(niceTicks(5, 5)).toMatchObject({ min: 0, max: 5 });
    expect(niceTicks(0, 0)).toMatchObject({ min: 0, max: 1 });
  });

  // Each of these looped forever (the step was too small to move the float), freezing the app.
  it("ends for values that differ by rounding noise only", { timeout: 2_000 }, () => {
    for (const [lo, hi] of [
      [0.1, 0.10000000000000002],
      [19.99, 19.990000000000002],
      [1e16, 10000000000000002],
      [-0.30000000000000004, -0.3],
    ] as const) {
      const t = niceTicks(lo, hi);
      expect(t.ticks.length).toBeGreaterThan(1);
      expect(t.ticks.length).toBeLessThanOrEqual(50);
      // One value: the axis runs from 0 to it (noise may stick out by a hair).
      const hair = 1e-9 * Math.max(Math.abs(lo), Math.abs(hi));
      expect(t.min).toBeLessThanOrEqual(Math.min(lo, hi, 0) + hair);
      expect(t.max).toBeGreaterThanOrEqual(Math.max(lo, hi, 0) - hair);
    }
    expect(niceTicks(0.1, 0.10000000000000002).ticks).toEqual([0, 0.05, 0.1]);
    // A line chart over such values lays out.
    const flat = layoutChart(
      data(
        "-- TablePlus LineChart x: k, y: v",
        [col("k", "text"), col("v", "float")],
        [
          [text("a"), { t: "float", v: 0.1 }],
          [text("b"), { t: "float", v: 0.10000000000000002 }],
        ],
      ),
      300,
      200,
    );
    expect(flat.ys[0]).toHaveLength(2);
  });

  it("keeps ticks of large values with many digits apart", () => {
    const t = niceTicks(1_700_000_000_000, 1_700_000_000_004);
    expect(t.ticks).toEqual([
      1_700_000_000_000, 1_700_000_000_001, 1_700_000_000_002, 1_700_000_000_003, 1_700_000_000_004,
    ]);
    expect(t.max - t.min).toBe(4);
    // Steps far below 1 still read without noise.
    expect(niceTicks(0, 0.0003).ticks).toEqual([0, 0.0001, 0.0002, 0.0003]);
    // Never more than the guard allows, never non-finite.
    expect(niceTicks(-Number.MAX_VALUE, Number.MAX_VALUE).ticks.every(Number.isFinite)).toBe(true);
    expect(niceTicks(0, 1, 1000).ticks.length).toBeLessThanOrEqual(4001);
  });

  it("formats ticks, values and shares", () => {
    expect(formatTick(1500, 500)).toBe("1,500");
    expect(formatTick(0.25, 0.25)).toBe("0.25");
    expect(formatTick(2_500_000, 500_000)).toBe("2.5M");
    expect(formatValue(1234.5678)).toBe("1,234.57");
    expect(formatValue(0.123456)).toBe("0.1235");
    expect(formatPercent(1, 3)).toBe("33.3%");
  });
});

describe("chart data", () => {
  it("reads numbers from numeric values and numeric text", () => {
    expect(toNumber(int(3))).toBe(3);
    expect(toNumber({ t: "bigInt", v: "9007199254740993" })).toBe(9007199254740992);
    expect(toNumber(text(" 1.5e3 "))).toBe(1500);
    expect(toNumber(text("abc"))).toBeNull();
    expect(toNumber({ t: "null" })).toBeNull();
  });

  it("builds one series per y column", () => {
    const d = data("-- TablePlus BarChart x: month, y: revenue, cost");
    expect(d.categories).toEqual(["Jan", "Feb", "Mar", "Apr"]);
    expect(d.series).toEqual([
      { name: "revenue", values: [120, 90, null, 150] },
      { name: "cost", values: [80.5, 95, 60, 70.25] },
    ]);
  });

  it("defaults to the first column against every numeric one", () => {
    const d = data("-- Tabler LineChart");
    expect(d.xName).toBe("month");
    expect(d.series.map((s) => s.name)).toEqual(["revenue", "cost"]);
  });

  it("says what is missing", () => {
    const d = parseDirective("-- TablePlus BarChart x: month, y: profit")!;
    expect(buildChartData(d, COLUMNS, ROWS)).toEqual({
      ok: false,
      error: 'The result has no column "profit".',
    });
    const none = parseDirective("-- TablePlus BarChart")!;
    expect(buildChartData(none, [col("a", "text")], []).ok).toBe(false);
  });

  it("orders a numeric x axis and samples long results", () => {
    const d = data(
      "-- TablePlus LineChart x: n, y: v",
      [col("n", "int"), col("v", "int")],
      [
        [int(3), int(30)],
        [int(1), int(10)],
        [int(2), int(20)],
      ],
    );
    expect(d.xValues).toEqual([1, 2, 3]);
    expect(d.series[0]!.values).toEqual([10, 20, 30]);
    expect(sampleIndexes(10, 4)).toEqual([0, 3, 6, 9]);
    const big = Array.from({ length: 12_000 }, (_, i) => [int(i), int(i)]);
    const sampled = data(
      "-- TablePlus LineChart x: n, y: v",
      [col("n", "int"), col("v", "int")],
      big,
    );
    expect(sampled.categories).toHaveLength(5_000);
    expect(sampled.notes[0]).toMatch(/Showing 5,000 points of 12,000 \(sampled\)/);
  });

  it("folds small pie slices into Other and leaves out non-positive values", () => {
    const rows = Array.from({ length: 10 }, (_, i) => [text(`s${i}`), int(i)]);
    const d = data("-- TablePlus PieChart x: k, y: v", [col("k", "text"), col("v", "int")], rows);
    expect(d.categories).toEqual(["s9", "s8", "s7", "s6", "s5", "s4", "s3", "Other"]);
    expect(d.series[0]!.values.at(-1)).toBe(3); // 2 + 1
    expect(d.folded).toBe(true);
    expect(d.notes.join(" ")).toMatch(/1 row without a positive value/);
  });
});

describe("chart scenes", () => {
  it("rounds a bar's data end only", () => {
    expect(barPath(10, 20, 100, 40)).toBe("M10 100V44A4 4 0 0 1 14 40H26A4 4 0 0 1 30 44V100Z");
    expect(barPath(10, 20, 100, 140)).toBe("M10 100V136A4 4 0 0 0 14 140H26A4 4 0 0 0 30 136V100Z");
    // A 1px bar: the rounding never exceeds its height.
    expect(barPath(0, 6, 50, 49)).toBe("M0 50V50A1 1 0 0 1 1 49H5A1 1 0 0 1 6 50V50Z");
  });

  it("snaps hover to the nearest point", () => {
    expect(nearestIndex([10, 20, 30], 24)).toBe(1);
    expect(nearestIndex([10, 20, 30], 26)).toBe(2);
    expect(nearestIndex([10, 20, 30], -5)).toBe(0);
    expect(nearestIndex([], 3)).toBeNull();
  });

  it("draws a grouped bar chart", () => {
    const scene = layoutChart(data("-- TablePlus BarChart x: month, y: revenue, cost"), 480, 260);
    expect(scene.tooltip(2)).toEqual({
      title: "Mar",
      rows: [
        { label: "revenue", value: "NULL", paint: "series-1", mark: "rect" },
        { label: "cost", value: "60", paint: "series-2", mark: "rect" },
      ],
    });
    expect(sceneToSvg(scene, "light")).toMatchSnapshot();
  });

  it("draws a line chart in dark colors", () => {
    const scene = layoutChart(data("-- TablePlus LineChart x: month, y: revenue"), 400, 220);
    // The NULL breaks the line; Apr stands alone and gets a marker.
    expect(scene.ys[0]![2]).toBeNull();
    expect(scene.nodes.filter((n) => n.t === "circle")).toHaveLength(1);
    expect(sceneToSvg(scene, "dark")).toMatchSnapshot();
  });

  it("draws a pie chart with its legend", () => {
    const scene = layoutChart(data("-- TablePlus PieChart x: month, y: cost"), 420, 240);
    expect(scene.slices).toHaveLength(4);
    expect(scene.tooltip(0)).toMatchObject({ title: "Feb", rows: [{ value: "95 (31.1%)" }] });
    expect(sceneToSvg(scene, "light")).toMatchSnapshot();
  });

  it("escapes labels from the database", () => {
    expect(xmlEscape(`<a href="x">&'`)).toBe("&lt;a href=&quot;x&quot;&gt;&amp;&apos;");
    const scene = layoutChart(
      data(
        "-- TablePlus BarChart x: k, y: v",
        [col("k", "text"), col("v", "int")],
        [[text("<script>"), int(1)]],
      ),
      300,
      200,
    );
    const svg = sceneToSvg(scene, "light");
    expect(svg).not.toContain("<script>");
    expect(svg).toContain("&lt;script&gt;");
  });

  it("never cuts a character in half and drops what XML cannot hold", () => {
    expect(clip("😀😀😀", 2)).toBe("😀…");
    expect(clip("😀😀", 2)).toBe("😀😀");
    expect(clip("abc", 1)).toBe("…");
    expect(xmlEscape("a\u0001b\u000bc\ud83d\t😀")).toBe("abc\t😀");
    const scene = layoutChart(
      data(
        "-- TablePlus BarChart x: k, y: v",
        [col("k", "text"), col("v", "int")],
        [
          [text("😀".repeat(40)), int(1)],
          [text("bell\u0007"), int(2)],
        ],
      ),
      300,
      200,
    );
    const svg = sceneToSvg(scene, "light");
    expect(svg).toContain("😀…");
    const xmlChar = (ch: string) => {
      const n = ch.codePointAt(0)!;
      return n >= 0x20 || ch === "\t" || ch === "\n" || ch === "\r";
    };
    expect([...svg].every(xmlChar)).toBe(true);
    expect(/\p{Cs}/u.test(svg)).toBe(false);
    // What Export PNG does with it: this threw URIError on a lone surrogate.
    expect(() => encodeURIComponent(svg)).not.toThrow();
    expect(chartFileName({ ...scene, title: `${"😀".repeat(100)}\u0001` }, "png")).toBe(
      `${"😀".repeat(80)}.png`,
    );
  });

  it("says when there is nothing to draw", () => {
    const empty = layoutChart(
      data("-- TablePlus BarChart x: month, y: revenue", COLUMNS, []),
      300,
      200,
    );
    expect(sceneToSvg(empty, "light")).toContain("No rows to chart");
  });
});
