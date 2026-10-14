import { describe, expect, it } from "vitest";
import {
  axisLabel,
  boundsOf,
  isGeographic,
  looksLikeWkt,
  niceStep,
  parseWkt,
  splitEwkt,
  svgPath,
} from "./wkt";

describe("splitEwkt / looksLikeWkt", () => {
  it("splits the SRID off EWKT", () => {
    expect(splitEwkt("SRID=4326;POINT(1 2)")).toEqual({ srid: 4326, wkt: "POINT(1 2)" });
    expect(splitEwkt("POINT(1 2)")).toEqual({ srid: null, wkt: "POINT(1 2)" });
  });
  it("tells WKT from hex", () => {
    expect(looksLikeWkt("SRID=4326;POINT(1 2)")).toBe(true);
    expect(looksLikeWkt("pointm(1 2 3)")).toBe(true);
    expect(looksLikeWkt("0101000020E6100000000000000000F03F")).toBe(false);
    expect(looksLikeWkt("hello")).toBe(false);
  });
});

describe("parseWkt", () => {
  it("reads points in every dimension", () => {
    expect(parseWkt("POINT(1 2)")?.shapes.points).toEqual([[1, 2]]);
    const z = parseWkt("SRID=4326;POINT Z (1 2 3)")!;
    expect([z.srid, z.type, z.dims, z.wkt]).toEqual([4326, "POINT", "Z", "POINT Z (1 2 3)"]);
    expect(z.shapes.points).toEqual([[1, 2]]);
    expect(parseWkt("POINT ZM (1 2 3 4)")?.dims).toBe("ZM");
    expect(parseWkt("POINTM(1 2 3)")?.dims).toBe("M");
    expect(parseWkt("POINT EMPTY")?.empty).toBe(true);
    expect(parseWkt("POINT(-1.5e2 .5)")?.shapes.points).toEqual([[-150, 0.5]]);
  });

  it("reads lines and polygons with holes", () => {
    expect(parseWkt("LINESTRING(0 0,1 1,2 0)")?.shapes.lines).toEqual([
      [
        [0, 0],
        [1, 1],
        [2, 0],
      ],
    ]);
    const p = parseWkt("POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,3 2,3 3,2 2))")!;
    expect(p.shapes.polygons).toHaveLength(1);
    expect(p.shapes.polygons[0]).toHaveLength(2);
    expect(parseWkt("POLYGON EMPTY")?.empty).toBe(true);
  });

  it("reads multi-geometries in both MULTIPOINT spellings", () => {
    expect(parseWkt("MULTIPOINT((1 2),(3 4))")?.shapes.points).toEqual([
      [1, 2],
      [3, 4],
    ]);
    expect(parseWkt("MULTIPOINT(1 2, 3 4)")?.shapes.points).toEqual([
      [1, 2],
      [3, 4],
    ]);
    expect(parseWkt("MULTILINESTRING((0 0,1 1),(2 2,3 3))")?.shapes.lines).toHaveLength(2);
    const mp = parseWkt("MULTIPOLYGON M (((0 0 1,1 0 1,0 1 1,0 0 1)),EMPTY)")!;
    expect(mp.dims).toBe("M");
    expect(mp.shapes.polygons).toHaveLength(1);
  });

  it("reads nested collections", () => {
    const g = parseWkt(
      "SRID=4326;GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1),GEOMETRYCOLLECTION(POLYGON((0 0,1 0,0 1,0 0))),GEOMETRYCOLLECTION EMPTY)",
    )!;
    expect(g.type).toBe("GEOMETRYCOLLECTION");
    expect(g.shapes.points).toHaveLength(1);
    expect(g.shapes.lines).toHaveLength(1);
    expect(g.shapes.polygons).toHaveLength(1);
  });

  it("samples circular arcs", () => {
    const c = parseWkt("CIRCULARSTRING(0 0,1 1,2 0)")!;
    const line = c.shapes.lines[0]!;
    expect(line.length).toBeGreaterThan(10);
    expect(line[0]).toEqual([0, 0]);
    expect(line[line.length - 1]).toEqual([2, 0]);
    // Every sample lies on the circle around (1, 0) with radius 1.
    for (const [x, y] of line) expect(Math.hypot(x - 1, y)).toBeCloseTo(1, 6);
    // …and on the upper half (the arc passes through (1, 1)).
    expect(line.every(([, y]) => y >= -1e-9)).toBe(true);
  });

  it("reads curves, surfaces and TINs", () => {
    expect(
      parseWkt("COMPOUNDCURVE(CIRCULARSTRING(0 0,1 1,2 0),(2 0,0 0))")?.shapes.lines,
    ).toHaveLength(1);
    expect(
      parseWkt("CURVEPOLYGON(CIRCULARSTRING(0 0,2 0,2 2,0 2,0 0))")?.shapes.polygons,
    ).toHaveLength(1);
    expect(
      parseWkt("MULTICURVE((0 0,5 5),CIRCULARSTRING(0 0,1 1,2 0))")?.shapes.lines,
    ).toHaveLength(2);
    expect(
      parseWkt("MULTISURFACE(CURVEPOLYGON(CIRCULARSTRING(0 0,2 0,0 0)),((0 0,1 0,0 1,0 0)))")
        ?.shapes.polygons,
    ).toHaveLength(2);
    expect(parseWkt("TIN Z (((0 0 0,0 1 0,1 0 0,0 0 0)))")?.shapes.polygons).toHaveLength(1);
    expect(
      parseWkt("POLYHEDRALSURFACE Z (((0 0 0,0 1 0,1 0 0,0 0 0)))")?.shapes.polygons,
    ).toHaveLength(1);
    expect(parseWkt("TRIANGLE((0 0,0 1,1 0,0 0))")?.shapes.polygons).toHaveLength(1);
  });

  it("refuses what is not WKT", () => {
    expect(parseWkt("0101000020E6100000")).toBeNull();
    expect(parseWkt("POINT(1)")).toBeNull();
    expect(parseWkt("POINT(1 2")).toBeNull();
    expect(parseWkt("POINT(1 2) trailing")).toBeNull();
    expect(parseWkt("BLOB(1 2)")).toBeNull();
  });
});

describe("drawing helpers", () => {
  it("measures the extent", () => {
    const g = parseWkt("MULTIPOINT((-10 5),(20 -3))")!;
    expect(boundsOf(g.shapes)).toEqual({ minX: -10, minY: -3, maxX: 20, maxY: 5 });
    expect(boundsOf(parseWkt("POINT EMPTY")!.shapes)).toBeNull();
  });
  it("builds SVG paths", () => {
    const g = parseWkt("POLYGON((0 0,1 0,0 1,0 0))")!;
    expect(svgPath(g.shapes, ([x, y]) => [x * 10, -y * 10], "polygons", 0)).toBe(
      "M0 0L10 0L0 -10L0 0Z",
    );
  });
  it("labels WGS 84 and geography as lon / lat", () => {
    expect(isGeographic(4326)).toBe(true);
    expect(isGeographic(3857)).toBe(false);
    expect(isGeographic(null, "geography")).toBe(true);
  });
});

describe("map grid", () => {
  it("picks round steps and labels lon / lat with hemispheres", () => {
    expect(niceStep(10)).toBe(1);
    expect(niceStep(0.03)).toBe(0.005);
    expect(niceStep(700)).toBe(100);
    expect(axisLabel(106.8, 0.1, "x", true)).toBe("106.8°E");
    expect(axisLabel(-6.25, 0.05, "y", true)).toBe("6.25°S");
    expect(axisLabel(0, 1, "x", true)).toBe("0°");
    expect(axisLabel(1500, 500, "x", false)).toBe("1500");
  });
});
