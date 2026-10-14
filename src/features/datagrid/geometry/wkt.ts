/**
 * WKT / EWKT parsing for the geometry previews (grid hover, Quick Look ▸ Map). The backend turns
 * PostGIS / MySQL spatial values into EWKT (`SRID=4326;POINT(1 2)`, see src-tauri/src/geometry.rs);
 * this reads it back into flat 2D shapes to draw. Z and M ordinates are kept out of the drawing
 * (the map is planar) but reported in `dims`. Curves are drawn through sampled arcs.
 */

export type Pt = [number, number];

/** What a geometry draws as: points, open lines and polygons (each a list of closed rings). */
export interface Shapes {
  points: Pt[];
  lines: Pt[][];
  polygons: Pt[][][];
}

export interface ParsedGeometry {
  /** The SRID of an EWKT value (`SRID=4326;…`), null without one. */
  srid: number | null;
  /** The top-level type keyword (`POINT`, `MULTIPOLYGON`…). */
  type: string;
  dims: "" | "Z" | "M" | "ZM";
  /** The WKT without the `SRID=…;` prefix. */
  wkt: string;
  shapes: Shapes;
  empty: boolean;
}

export interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

const TYPES = new Set([
  "POINT",
  "LINESTRING",
  "POLYGON",
  "MULTIPOINT",
  "MULTILINESTRING",
  "MULTIPOLYGON",
  "GEOMETRYCOLLECTION",
  "CIRCULARSTRING",
  "COMPOUNDCURVE",
  "CURVEPOLYGON",
  "MULTICURVE",
  "MULTISURFACE",
  "POLYHEDRALSURFACE",
  "TIN",
  "TRIANGLE",
]);

const SRID_PREFIX = /^\s*SRID=(\d+)\s*;\s*/i;

/** Splits EWKT into its SRID (null when absent) and the WKT. */
export function splitEwkt(text: string): { srid: number | null; wkt: string } {
  const m = SRID_PREFIX.exec(text);
  return m ? { srid: Number(m[1]), wkt: text.slice(m[0].length) } : { srid: null, wkt: text };
}

/** Whether `text` reads as WKT / EWKT (a known type keyword up front) rather than hex or junk. */
export function looksLikeWkt(text: string): boolean {
  const word = /^[A-Za-z]+/.exec(splitEwkt(text).wkt.trimStart())?.[0]?.toUpperCase() ?? "";
  return TYPES.has(word) || TYPES.has(word.replace(/(ZM|Z|M)$/, ""));
}

type Tok = { k: "word"; v: string } | { k: "num"; v: number } | { k: "(" | ")" | "," };

const TOKEN = /\s*(?:([A-Za-z]+)|([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)|([(),]))/y;

function tokenize(text: string): Tok[] | null {
  const out: Tok[] = [];
  TOKEN.lastIndex = 0;
  let at = 0;
  while (at < text.length) {
    if (/^\s*$/.test(text.slice(at))) break;
    TOKEN.lastIndex = at;
    const m = TOKEN.exec(text);
    if (!m) return null;
    at = TOKEN.lastIndex;
    if (m[1] !== undefined) {
      const w = m[1].toUpperCase();
      if (w === "NAN") out.push({ k: "num", v: NaN });
      else if (w === "INF" || w === "INFINITY") out.push({ k: "num", v: Infinity });
      else out.push({ k: "word", v: w });
    } else if (m[2] !== undefined) out.push({ k: "num", v: Number(m[2]) });
    else out.push({ k: m[3] as "(" | ")" | "," });
  }
  return out;
}

class ParseError extends Error {}

/** Most points sampled on one circular arc. */
const ARC_STEPS = 24;

/** The arc through `a`, `b`, `c` (SQL-MM circular string segment), sampled; a line when collinear. */
function arcPoints(a: Pt, b: Pt, c: Pt): Pt[] {
  const circle = (ux: number, uy: number, from: number, sweep: number): Pt[] => {
    const r = Math.hypot(a[0] - ux, a[1] - uy);
    return Array.from({ length: ARC_STEPS }, (_, i) => {
      const t = from + (sweep * (i + 1)) / ARC_STEPS;
      return [ux + r * Math.cos(t), uy + r * Math.sin(t)] as Pt;
    });
  };
  // `a` = `c`: a full circle whose diameter is a–b.
  if (a[0] === c[0] && a[1] === c[1]) {
    const ux = (a[0] + b[0]) / 2;
    const uy = (a[1] + b[1]) / 2;
    if (ux === a[0] && uy === a[1]) return [c];
    return circle(ux, uy, Math.atan2(a[1] - uy, a[0] - ux), 2 * Math.PI);
  }
  const d = 2 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
  if (Math.abs(d) < 1e-12) return [b, c];
  const sq = (p: Pt) => p[0] * p[0] + p[1] * p[1];
  const ux = (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d;
  const uy = (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d;
  const from = Math.atan2(a[1] - uy, a[0] - ux);
  // a → b → c counter-clockwise (d > 0) sweeps a positive angle, clockwise a negative one.
  let sweep = Math.atan2(c[1] - uy, c[0] - ux) - from;
  if (d > 0) while (sweep <= 0) sweep += 2 * Math.PI;
  else while (sweep >= 0) sweep -= 2 * Math.PI;
  const out = circle(ux, uy, from, sweep);
  out[out.length - 1] = c;
  return out;
}

/** A circular string's coordinates as a polyline (every arc sampled). */
function densify(coords: Pt[]): Pt[] {
  if (coords.length < 3) return coords;
  const out: Pt[] = [coords[0]!];
  for (let i = 0; i + 2 < coords.length; i += 2)
    out.push(...arcPoints(coords[i]!, coords[i + 1]!, coords[i + 2]!));
  return out;
}

class Parser {
  private i = 0;
  readonly shapes: Shapes = { points: [], lines: [], polygons: [] };

  constructor(private readonly toks: Tok[]) {}

  done(): boolean {
    return this.i >= this.toks.length;
  }

  private peek(): Tok | undefined {
    return this.toks[this.i];
  }

  private next(): Tok {
    const t = this.toks[this.i++];
    if (!t) throw new ParseError("unexpected end");
    return t;
  }

  private expect(k: "(" | ")" | ","): void {
    if (this.next().k !== k) throw new ParseError(`expected ${k}`);
  }

  private isWord(w: string): boolean {
    const t = this.peek();
    return t?.k === "word" && t.v === w;
  }

  /** `EMPTY` (consumed) or not. */
  private empty(): boolean {
    if (!this.isWord("EMPTY")) return false;
    this.i++;
    return true;
  }

  /** `TYPE [Z|M|ZM]` (also `POINTZ` / `POINTM` EWKT spellings). */
  header(): { type: string; dims: ParsedGeometry["dims"] } {
    const t = this.next();
    if (t.k !== "word") throw new ParseError("expected a geometry type");
    let type = t.v;
    let dims: ParsedGeometry["dims"] = "";
    if (!TYPES.has(type)) {
      const m = /^(.*?)(ZM|Z|M)$/.exec(type);
      if (!m || !TYPES.has(m[1]!)) throw new ParseError(`unknown type ${type}`);
      type = m[1]!;
      dims = m[2] as ParsedGeometry["dims"];
    }
    const d = this.peek();
    if (d?.k === "word" && (d.v === "Z" || d.v === "M" || d.v === "ZM")) {
      dims = d.v;
      this.i++;
    }
    return { type, dims };
  }

  private coord(): Pt {
    const nums: number[] = [];
    while (this.peek()?.k === "num") nums.push((this.next() as { v: number }).v);
    if (nums.length < 2) throw new ParseError("expected a coordinate");
    return [nums[0]!, nums[1]!];
  }

  /** `(x y, x y, …)` or `EMPTY`. */
  private coordList(): Pt[] {
    if (this.empty()) return [];
    this.expect("(");
    const out: Pt[] = [this.coord()];
    while (this.peek()?.k === ",") {
      this.i++;
      out.push(this.coord());
    }
    this.expect(")");
    return out;
  }

  /** `((ring), (ring))` or `EMPTY`. */
  private ringList(): Pt[][] {
    if (this.empty()) return [];
    this.expect("(");
    const out: Pt[][] = [this.coordList()];
    while (this.peek()?.k === ",") {
      this.i++;
      out.push(this.coordList());
    }
    this.expect(")");
    return out;
  }

  /** `( item, item )` or `EMPTY`, each item read by `item`. */
  private list(item: () => void): void {
    if (this.empty()) return;
    this.expect("(");
    item();
    while (this.peek()?.k === ",") {
      this.i++;
      item();
    }
    this.expect(")");
  }

  /** A curve member: a bare `(x y, …)` line or a tagged CIRCULARSTRING / COMPOUNDCURVE / LINESTRING. */
  private curve(): Pt[] {
    if (this.peek()?.k === "(") return this.coordList();
    if (this.empty()) return [];
    const { type } = this.header();
    if (type === "LINESTRING") return this.coordList();
    if (type === "CIRCULARSTRING") return densify(this.coordList());
    if (type === "COMPOUNDCURVE") {
      const out: Pt[] = [];
      this.list(() => {
        const part = this.curve();
        // Members share their end points.
        out.push(...(out.length > 0 ? part.slice(1) : part));
      });
      return out;
    }
    throw new ParseError(`unexpected ${type} in a curve`);
  }

  private polygon(rings: Pt[][]): void {
    const kept = rings.filter((r) => r.length > 0);
    if (kept.length > 0) this.shapes.polygons.push(kept);
  }

  private line(coords: Pt[]): void {
    if (coords.length === 1) this.shapes.points.push(coords[0]!);
    else if (coords.length > 1) this.shapes.lines.push(coords);
  }

  /** A curve polygon: its rings are curves. */
  private curvePolygon(): void {
    const rings: Pt[][] = [];
    this.list(() => rings.push(this.curve()));
    this.polygon(rings);
  }

  /** The body of `type` (after its header). */
  body(type: string): void {
    switch (type) {
      case "POINT": {
        if (this.empty()) return;
        this.expect("(");
        if (this.empty()) {
          this.expect(")");
          return;
        }
        const p = this.coord();
        this.expect(")");
        if (Number.isFinite(p[0]) && Number.isFinite(p[1])) this.shapes.points.push(p);
        return;
      }
      case "LINESTRING":
        return this.line(this.coordList());
      case "CIRCULARSTRING":
        return this.line(densify(this.coordList()));
      case "POLYGON":
      case "TRIANGLE":
        return this.polygon(this.ringList());
      case "MULTIPOINT":
        return this.list(() => {
          if (this.empty()) return;
          if (this.peek()?.k === "(") {
            this.i++;
            this.shapes.points.push(this.coord());
            this.expect(")");
          } else this.shapes.points.push(this.coord());
        });
      case "MULTILINESTRING":
        return this.list(() => this.line(this.coordList()));
      case "MULTIPOLYGON":
      case "POLYHEDRALSURFACE":
      case "TIN":
        return this.list(() => this.polygon(this.ringList()));
      case "COMPOUNDCURVE": {
        const out: Pt[] = [];
        this.list(() => {
          const part = this.curve();
          out.push(...(out.length > 0 ? part.slice(1) : part));
        });
        return this.line(out);
      }
      case "MULTICURVE":
        return this.list(() => this.line(this.curve()));
      case "CURVEPOLYGON":
        return this.curvePolygon();
      case "MULTISURFACE":
        return this.list(() => {
          if (this.peek()?.k === "(") return this.polygon(this.ringList());
          if (this.empty()) return;
          const { type: member } = this.header();
          if (member === "CURVEPOLYGON") this.curvePolygon();
          else if (member === "POLYGON") this.polygon(this.ringList());
          else throw new ParseError(`unexpected ${member} in a surface`);
        });
      case "GEOMETRYCOLLECTION":
        return this.list(() => {
          const { type: member } = this.header();
          this.body(member);
        });
    }
  }
}

/** Parses WKT or EWKT; null when it is not WKT (hex, other text) or is malformed. */
export function parseWkt(text: string): ParsedGeometry | null {
  const { srid, wkt } = splitEwkt(text);
  if (!looksLikeWkt(wkt)) return null;
  const toks = tokenize(wkt);
  if (!toks) return null;
  try {
    const p = new Parser(toks);
    const { type, dims } = p.header();
    p.body(type);
    if (!p.done()) return null;
    const s = p.shapes;
    return {
      srid,
      type,
      dims,
      wkt: wkt.trim(),
      shapes: s,
      empty: s.points.length === 0 && s.lines.length === 0 && s.polygons.length === 0,
    };
  } catch (e) {
    if (e instanceof ParseError) return null;
    throw e;
  }
}

/** Every drawn coordinate. */
function* allPoints(s: Shapes): Generator<Pt> {
  yield* s.points;
  for (const l of s.lines) yield* l;
  for (const p of s.polygons) for (const r of p) yield* r;
}

/** The extent of `s`; null when it draws nothing finite. */
export function boundsOf(s: Shapes): Bounds | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const [x, y] of allPoints(s)) {
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    if (x < minX) minX = x;
    if (y < minY) minY = y;
    if (x > maxX) maxX = x;
    if (y > maxY) maxY = y;
  }
  return minX === Infinity ? null : { minX, minY, maxX, maxY };
}

/** Coordinates count (for the Map view's caption). */
export function pointCount(s: Shapes): number {
  let n = 0;
  for (const _ of allPoints(s)) n++;
  return n;
}

/** Whether the SRID is a lon/lat (geographic) one we label as such: WGS 84 and its kin. */
export function isGeographic(srid: number | null, typeName?: string): boolean {
  if (typeName && /geography/i.test(typeName)) return true;
  return srid === 4326 || srid === 4269 || srid === 4258 || srid === 4019;
}

/** An SVG path (`M…L…Z`) of lines and polygon rings through `project`. */
export function svgPath(
  s: Shapes,
  project: (p: Pt) => Pt,
  what: "lines" | "polygons",
  digits = 2,
): string {
  const f = (n: number) => (Number.isFinite(n) ? n.toFixed(digits) : "0");
  const seg = (pts: Pt[], close: boolean) =>
    pts
      .map((p, i) => {
        const [x, y] = project(p);
        return `${i === 0 ? "M" : "L"}${f(x)} ${f(y)}`;
      })
      .join("") + (close ? "Z" : "");
  if (what === "lines") return s.lines.map((l) => seg(l, false)).join("");
  return s.polygons.map((poly) => poly.map((r) => seg(r, true)).join("")).join("");
}

/** A round step for about `target` grid lines over `span` (1, 2 or 5 × 10ⁿ). */
export function niceStep(span: number, target = 8): number {
  if (!(span > 0) || !Number.isFinite(span)) return 1;
  const raw = span / target;
  const p = 10 ** Math.floor(Math.log10(raw));
  const f = raw / p;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * p;
}

/** A grid label: lon / lat with a hemisphere (`106.8°E`), plain numbers otherwise. */
export function axisLabel(v: number, step: number, axis: "x" | "y", geo: boolean): string {
  const digits = Math.max(0, Math.min(10, -Math.floor(Math.log10(step) + 1e-9)));
  const n = Math.abs(v) < step / 1e6 ? 0 : v;
  if (!geo) return n.toFixed(digits);
  const hemi = n === 0 ? "" : axis === "x" ? (n < 0 ? "W" : "E") : n < 0 ? "S" : "N";
  return `${Math.abs(n).toFixed(digits)}°${hemi}`;
}
