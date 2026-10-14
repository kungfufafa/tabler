//! Spatial values for the data grid: WKB, PostGIS EWKB (SRID, Z / M flags) and MySQL's internal
//! format (SRID + WKB) decoded and written as WKT.
//!
//! The databases hand geometries over as binary: PostGIS's text output (`geometry::text`,
//! `geography::text`) is hex EWKB, MySQL sends its internal format. While "Automatically convert
//! PostGIS geometry to WKT" is on (Settings ▸ General ▸ Data Decoder; on by default, as in
//! TablePlus) the drivers turn them into WKT instead: `SRID=4326;POINT(1 2)` when the value has
//! an SRID, which PostGIS reads back as the same geometry. Values that do not parse are left as
//! they were (hex), so nothing is lost.

use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::value::decode_hex;

static CONVERT_TO_WKT: AtomicBool = AtomicBool::new(true);

/// Turns the WKT conversion on or off for every value decoded from now on (the renderer sends
/// the preference at startup and whenever it changes).
pub fn set_convert_to_wkt(on: bool) {
    CONVERT_TO_WKT.store(on, Ordering::Relaxed);
}

/// Whether the drivers convert spatial values to WKT (see `set_convert_to_wkt`).
pub fn convert_to_wkt() -> bool {
    CONVERT_TO_WKT.load(Ordering::Relaxed)
}

/// Coordinate dimensions: XY, XYZ (3D), XYM (measured), XYZM (4D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dims {
    Xy,
    Xyz,
    Xym,
    Xyzm,
}

impl Dims {
    fn new(z: bool, m: bool) -> Dims {
        match (z, m) {
            (false, false) => Dims::Xy,
            (true, false) => Dims::Xyz,
            (false, true) => Dims::Xym,
            (true, true) => Dims::Xyzm,
        }
    }

    /// Ordinates per coordinate.
    pub fn ordinates(self) -> usize {
        match self {
            Dims::Xy => 2,
            Dims::Xyz | Dims::Xym => 3,
            Dims::Xyzm => 4,
        }
    }

    /// The ISO WKT dimension tag (`POINT Z (…)`); empty for XY.
    fn tag(self) -> &'static str {
        match self {
            Dims::Xy => "",
            Dims::Xyz => " Z",
            Dims::Xym => " M",
            Dims::Xyzm => " ZM",
        }
    }
}

/// Geometry types (OGC / SQL-MM codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
    CircularString,
    CompoundCurve,
    CurvePolygon,
    MultiCurve,
    MultiSurface,
    PolyhedralSurface,
    Tin,
    Triangle,
}

impl Kind {
    fn from_code(code: u32) -> Option<Kind> {
        Some(match code {
            1 => Kind::Point,
            2 => Kind::LineString,
            3 => Kind::Polygon,
            4 => Kind::MultiPoint,
            5 => Kind::MultiLineString,
            6 => Kind::MultiPolygon,
            7 => Kind::GeometryCollection,
            8 => Kind::CircularString,
            9 => Kind::CompoundCurve,
            10 => Kind::CurvePolygon,
            11 => Kind::MultiCurve,
            12 => Kind::MultiSurface,
            15 => Kind::PolyhedralSurface,
            16 => Kind::Tin,
            17 => Kind::Triangle,
            _ => return None,
        })
    }

    /// The WKT keyword.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Point => "POINT",
            Kind::LineString => "LINESTRING",
            Kind::Polygon => "POLYGON",
            Kind::MultiPoint => "MULTIPOINT",
            Kind::MultiLineString => "MULTILINESTRING",
            Kind::MultiPolygon => "MULTIPOLYGON",
            Kind::GeometryCollection => "GEOMETRYCOLLECTION",
            Kind::CircularString => "CIRCULARSTRING",
            Kind::CompoundCurve => "COMPOUNDCURVE",
            Kind::CurvePolygon => "CURVEPOLYGON",
            Kind::MultiCurve => "MULTICURVE",
            Kind::MultiSurface => "MULTISURFACE",
            Kind::PolyhedralSurface => "POLYHEDRALSURFACE",
            Kind::Tin => "TIN",
            Kind::Triangle => "TRIANGLE",
        }
    }

    /// The member type written without its keyword inside this type (`MULTIPOINT((1 2))`,
    /// `MULTICURVE((0 0,1 1),CIRCULARSTRING(…))`); collections tag every member.
    fn bare_member(self) -> Option<Kind> {
        match self {
            Kind::MultiPoint => Some(Kind::Point),
            Kind::MultiLineString | Kind::CompoundCurve | Kind::CurvePolygon | Kind::MultiCurve => {
                Some(Kind::LineString)
            }
            Kind::MultiPolygon | Kind::MultiSurface | Kind::PolyhedralSurface => {
                Some(Kind::Polygon)
            }
            Kind::Tin => Some(Kind::Triangle),
            _ => None,
        }
    }
}

/// One coordinate: x, y and then z and / or m (see `Dims`).
pub type Coord = Vec<f64>;

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A point; `None` is `POINT EMPTY` (NaN coordinates in WKB).
    Point(Option<Coord>),
    /// LineString, CircularString.
    Line(Vec<Coord>),
    /// Polygon, Triangle: the rings, outer first.
    Rings(Vec<Vec<Coord>>),
    /// Multi-geometries, collections, compound curves, curve polygons, polyhedral surfaces, TINs.
    Parts(Vec<Geometry>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub kind: Kind,
    pub dims: Dims,
    /// The SRID an EWKB / MySQL value carries (members of a collection have none).
    pub srid: Option<u32>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeometryError {
    Truncated,
    ByteOrder(u8),
    UnknownType(u32),
    TooDeep,
    TrailingBytes,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeometryError::Truncated => f.write_str("the value ends too early"),
            GeometryError::ByteOrder(b) => write!(f, "unknown byte order {b}"),
            GeometryError::UnknownType(t) => write!(f, "unknown geometry type {t:#x}"),
            GeometryError::TooDeep => f.write_str("geometry collections nest too deeply"),
            GeometryError::TrailingBytes => f.write_str("unexpected bytes after the geometry"),
        }
    }
}

impl std::error::Error for GeometryError {}

/// Collections nested deeper than this are refused (a crafted value cannot exhaust the stack).
const MAX_DEPTH: usize = 32;

/// EWKB type flags (PostGIS); ISO WKB adds 1000 / 2000 / 3000 to the type code instead.
const EWKB_Z: u32 = 0x8000_0000;
const EWKB_M: u32 = 0x4000_0000;
const EWKB_SRID: u32 = 0x2000_0000;

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], GeometryError> {
        let bytes = self
            .buf
            .get(self.pos..self.pos + N)
            .ok_or(GeometryError::Truncated)?;
        self.pos += N;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, GeometryError> {
        Ok(self.take::<1>()?[0])
    }

    fn u32(&mut self, le: bool) -> Result<u32, GeometryError> {
        let b = self.take::<4>()?;
        Ok(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn f64(&mut self, le: bool) -> Result<f64, GeometryError> {
        let b = self.take::<8>()?;
        Ok(if le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    }

    /// An element count, refused when that many items of at least `min_bytes` cannot fit in
    /// what is left (no huge allocation from a corrupt count).
    fn count(&mut self, le: bool, min_bytes: usize) -> Result<usize, GeometryError> {
        let n = self.u32(le)? as usize;
        if n.saturating_mul(min_bytes) > self.remaining() {
            return Err(GeometryError::Truncated);
        }
        Ok(n)
    }

    fn coord(&mut self, le: bool, dims: Dims) -> Result<Coord, GeometryError> {
        (0..dims.ordinates()).map(|_| self.f64(le)).collect()
    }

    fn coords(&mut self, le: bool, dims: Dims) -> Result<Vec<Coord>, GeometryError> {
        let n = self.count(le, dims.ordinates() * 8)?;
        (0..n).map(|_| self.coord(le, dims)).collect()
    }
}

/// Parses WKB, ISO WKB (Z / M / ZM type codes) or PostGIS EWKB (Z / M / SRID flags).
pub fn parse_wkb(bytes: &[u8]) -> Result<Geometry, GeometryError> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let g = read_geometry(&mut r, 0)?;
    if r.remaining() != 0 {
        return Err(GeometryError::TrailingBytes);
    }
    Ok(g)
}

/// Parses MySQL's internal geometry format: a 4-byte little-endian SRID, then WKB.
pub fn parse_mysql(bytes: &[u8]) -> Result<Geometry, GeometryError> {
    let (srid, wkb) = bytes
        .split_first_chunk::<4>()
        .ok_or(GeometryError::Truncated)?;
    let mut g = parse_wkb(wkb)?;
    let srid = u32::from_le_bytes(*srid);
    g.srid = (srid != 0).then_some(srid);
    Ok(g)
}

fn read_geometry(r: &mut Reader<'_>, depth: usize) -> Result<Geometry, GeometryError> {
    if depth > MAX_DEPTH {
        return Err(GeometryError::TooDeep);
    }
    let le = match r.u8()? {
        0 => false,
        1 => true,
        other => return Err(GeometryError::ByteOrder(other)),
    };
    let raw = r.u32(le)?;
    let mut z = raw & EWKB_Z != 0;
    let mut m = raw & EWKB_M != 0;
    let code = raw & 0x0FFF_FFFF;
    match code / 1000 {
        0 => {}
        1 => z = true,
        2 => m = true,
        3 => (z, m) = (true, true),
        _ => return Err(GeometryError::UnknownType(raw)),
    }
    let kind = Kind::from_code(code % 1000).ok_or(GeometryError::UnknownType(raw))?;
    let srid = if raw & EWKB_SRID != 0 {
        Some(r.u32(le)?)
    } else {
        None
    };
    let dims = Dims::new(z, m);
    let body = match kind {
        Kind::Point => {
            let c = r.coord(le, dims)?;
            Body::Point((!c.iter().all(|v| v.is_nan())).then_some(c))
        }
        Kind::LineString | Kind::CircularString => Body::Line(r.coords(le, dims)?),
        Kind::Polygon | Kind::Triangle => {
            let n = r.count(le, 4)?;
            Body::Rings(
                (0..n)
                    .map(|_| r.coords(le, dims))
                    .collect::<Result<_, _>>()?,
            )
        }
        _ => {
            // Every member has its own header (byte order + type): at least 5 bytes.
            let n = r.count(le, 5)?;
            Body::Parts(
                (0..n)
                    .map(|_| read_geometry(r, depth + 1))
                    .collect::<Result<_, _>>()?,
            )
        }
    };
    Ok(Geometry {
        kind,
        dims,
        srid,
        body,
    })
}

impl Geometry {
    /// ISO WKT: `POINT(1 2)`, `POINT Z (1 2 3)`, `MULTIPOINT((1 2),(3 4))`, `POLYGON EMPTY`.
    pub fn to_wkt(&self) -> String {
        let mut out = String::new();
        write_tagged(&mut out, self);
        out
    }

    /// `SRID=4326;` and the WKT when the value has an SRID (EWKT, which PostGIS reads back),
    /// else the plain WKT.
    pub fn to_ewkt(&self) -> String {
        match self.srid {
            Some(srid) if srid != 0 => format!("SRID={srid};{}", self.to_wkt()),
            _ => self.to_wkt(),
        }
    }

    fn is_empty(&self) -> bool {
        match &self.body {
            Body::Point(c) => c.is_none(),
            Body::Line(c) => c.is_empty(),
            Body::Rings(r) => r.is_empty(),
            Body::Parts(p) => p.is_empty(),
        }
    }
}

fn write_tagged(out: &mut String, g: &Geometry) {
    out.push_str(g.kind.name());
    out.push_str(g.dims.tag());
    if g.is_empty() {
        out.push_str(" EMPTY");
        return;
    }
    if g.dims != Dims::Xy {
        out.push(' ');
    }
    write_body(out, g);
}

/// The parenthesized part of `g` (what follows its keyword).
fn write_body(out: &mut String, g: &Geometry) {
    match &g.body {
        Body::Point(None) => out.push_str("EMPTY"),
        Body::Point(Some(c)) => {
            out.push('(');
            write_coord(out, c);
            out.push(')');
        }
        Body::Line(coords) => write_coords(out, coords),
        Body::Rings(rings) => {
            out.push('(');
            for (i, ring) in rings.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_coords(out, ring);
            }
            out.push(')');
        }
        Body::Parts(parts) => {
            let bare = g.kind.bare_member();
            out.push('(');
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if bare == Some(part.kind) {
                    if part.is_empty() {
                        out.push_str("EMPTY");
                    } else {
                        write_body(out, part);
                    }
                } else {
                    write_tagged(out, part);
                }
            }
            out.push(')');
        }
    }
}

fn write_coords(out: &mut String, coords: &[Coord]) {
    if coords.is_empty() {
        out.push_str("EMPTY");
        return;
    }
    out.push('(');
    for (i, c) in coords.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_coord(out, c);
    }
    out.push(')');
}

fn write_coord(out: &mut String, c: &[f64]) {
    for (i, v) in c.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        write_number(out, *v);
    }
}

/// The shortest text that reads back as the same double (`1`, `-0.5`, `1e-7`, `2.5e20`).
fn write_number(out: &mut String, v: f64) {
    if v == 0.0 {
        out.push('0');
        return;
    }
    let a = v.abs();
    if (1e-5..1e17).contains(&a) || !v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        let _ = write!(out, "{v:e}");
    }
}

/// PostGIS text output (hex EWKB) as EWKT; `None` when it is not hex EWKB.
pub fn ewkb_hex_to_ewkt(hex: &str) -> Option<String> {
    let bytes = decode_hex(hex)?;
    parse_wkb(&bytes).ok().map(|g| g.to_ewkt())
}

/// A MySQL internal-format value (SRID + WKB) as EWKT; `None` when it does not parse.
pub fn mysql_to_ewkt(bytes: &[u8]) -> Option<String> {
    parse_mysql(bytes).ok().map(|g| g.to_ewkt())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WKB writer for the tests: byte order, type code (with EWKB flags or ISO offsets).
    struct W {
        le: bool,
        out: Vec<u8>,
    }

    impl W {
        fn new(le: bool) -> W {
            W {
                le,
                out: Vec::new(),
            }
        }
        fn header(&mut self, ty: u32) -> &mut Self {
            self.out.push(self.le as u8);
            self.u32(ty)
        }
        fn u32(&mut self, v: u32) -> &mut Self {
            let b = if self.le {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            };
            self.out.extend_from_slice(&b);
            self
        }
        fn f(&mut self, vals: &[f64]) -> &mut Self {
            for v in vals {
                let b = if self.le {
                    v.to_le_bytes()
                } else {
                    v.to_be_bytes()
                };
                self.out.extend_from_slice(&b);
            }
            self
        }
        fn bytes(&self) -> Vec<u8> {
            self.out.clone()
        }
    }

    fn wkt(bytes: &[u8]) -> String {
        parse_wkb(bytes).unwrap().to_ewkt()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    #[test]
    fn postgis_point_with_srid() {
        // SELECT 'SRID=4326;POINT(1 2)'::geometry::text
        assert_eq!(
            ewkb_hex_to_ewkt("0101000020E6100000000000000000F03F0000000000000040").as_deref(),
            Some("SRID=4326;POINT(1 2)")
        );
        // Lower-case hex and no SRID.
        assert_eq!(
            ewkb_hex_to_ewkt("0101000000000000000000f03f0000000000000040").as_deref(),
            Some("POINT(1 2)")
        );
    }

    #[test]
    fn point_dimensions_ewkb_flags() {
        let z = W::new(true).header(1 | EWKB_Z).f(&[1.0, 2.0, 3.0]).bytes();
        assert_eq!(wkt(&z), "POINT Z (1 2 3)");
        let m = W::new(true).header(1 | EWKB_M).f(&[1.0, 2.0, 4.0]).bytes();
        assert_eq!(wkt(&m), "POINT M (1 2 4)");
        let zm = W::new(true)
            .header(1 | EWKB_Z | EWKB_M | EWKB_SRID)
            .u32(3857)
            .f(&[1.0, 2.0, 3.0, 4.0])
            .bytes();
        assert_eq!(wkt(&zm), "SRID=3857;POINT ZM (1 2 3 4)");
        let g = parse_wkb(&zm).unwrap();
        assert_eq!((g.dims, g.srid), (Dims::Xyzm, Some(3857)));
    }

    #[test]
    fn point_dimensions_iso_codes_both_byte_orders() {
        for le in [true, false] {
            let z = W::new(le).header(1001).f(&[1.0, 2.0, 3.0]).bytes();
            assert_eq!(wkt(&z), "POINT Z (1 2 3)");
            let m = W::new(le).header(2001).f(&[1.0, 2.0, 4.0]).bytes();
            assert_eq!(wkt(&m), "POINT M (1 2 4)");
            let zm = W::new(le).header(3001).f(&[1.0, 2.0, 3.0, 4.0]).bytes();
            assert_eq!(wkt(&zm), "POINT ZM (1 2 3 4)");
        }
    }

    #[test]
    fn big_endian_matches_little_endian() {
        let le = W::new(true).header(1).f(&[-71.06, 42.36]).bytes();
        let be = W::new(false).header(1).f(&[-71.06, 42.36]).bytes();
        assert_eq!(wkt(&le), "POINT(-71.06 42.36)");
        assert_eq!(wkt(&be), wkt(&le));
    }

    #[test]
    fn empty_point_is_nan() {
        let p = W::new(true).header(1).f(&[f64::NAN, f64::NAN]).bytes();
        assert_eq!(wkt(&p), "POINT EMPTY");
        let p = W::new(true).header(1001).f(&[f64::NAN; 3]).bytes();
        assert_eq!(wkt(&p), "POINT Z EMPTY");
    }

    #[test]
    fn linestring_and_empty() {
        let l = W::new(true)
            .header(2)
            .u32(3)
            .f(&[0.0, 0.0, 1.0, 1.0, 2.5, -1.0])
            .bytes();
        assert_eq!(wkt(&l), "LINESTRING(0 0,1 1,2.5 -1)");
        let e = W::new(true).header(2).u32(0).bytes();
        assert_eq!(wkt(&e), "LINESTRING EMPTY");
        let z = W::new(true)
            .header(2 | EWKB_Z)
            .u32(2)
            .f(&[0.0, 0.0, 5.0, 1.0, 1.0, 6.0])
            .bytes();
        assert_eq!(wkt(&z), "LINESTRING Z (0 0 5,1 1 6)");
        let m = W::new(true)
            .header(2002)
            .u32(2)
            .f(&[0.0, 0.0, 5.0, 1.0, 1.0, 6.0])
            .bytes();
        assert_eq!(wkt(&m), "LINESTRING M (0 0 5,1 1 6)");
    }

    #[test]
    fn polygon_with_hole() {
        let mut w = W::new(true);
        w.header(3 | EWKB_SRID).u32(4326).u32(2);
        w.u32(5)
            .f(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0, 0.0, 0.0]);
        w.u32(4).f(&[2.0, 2.0, 3.0, 2.0, 3.0, 3.0, 2.0, 2.0]);
        assert_eq!(
            wkt(&w.bytes()),
            "SRID=4326;POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,3 2,3 3,2 2))"
        );
        let e = W::new(true).header(3).u32(0).bytes();
        assert_eq!(wkt(&e), "POLYGON EMPTY");
        let mut zm = W::new(true);
        zm.header(3003).u32(1).u32(4);
        zm.f(&[
            0.0, 0.0, 1.0, 9.0, 1.0, 0.0, 1.0, 9.0, 0.0, 1.0, 1.0, 9.0, 0.0, 0.0, 1.0, 9.0,
        ]);
        assert_eq!(
            wkt(&zm.bytes()),
            "POLYGON ZM ((0 0 1 9,1 0 1 9,0 1 1 9,0 0 1 9))"
        );
    }

    #[test]
    fn multipoint() {
        let mut w = W::new(true);
        w.header(4).u32(2);
        w.header(1).f(&[1.0, 2.0]);
        w.header(1).f(&[3.0, 4.0]);
        assert_eq!(wkt(&w.bytes()), "MULTIPOINT((1 2),(3 4))");
        let mut z = W::new(false);
        z.header(4 | EWKB_Z).u32(1);
        z.header(1 | EWKB_Z).f(&[1.0, 2.0, 3.0]);
        assert_eq!(wkt(&z.bytes()), "MULTIPOINT Z ((1 2 3))");
    }

    #[test]
    fn multilinestring_and_multipolygon() {
        let mut w = W::new(true);
        w.header(5).u32(2);
        w.header(2).u32(2).f(&[0.0, 0.0, 1.0, 1.0]);
        w.header(2).u32(2).f(&[2.0, 2.0, 3.0, 3.0]);
        assert_eq!(wkt(&w.bytes()), "MULTILINESTRING((0 0,1 1),(2 2,3 3))");

        let mut p = W::new(true);
        p.header(6 | EWKB_M).u32(2);
        p.header(3 | EWKB_M)
            .u32(1)
            .u32(4)
            .f(&[0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
        p.header(3 | EWKB_M).u32(0);
        assert_eq!(
            wkt(&p.bytes()),
            "MULTIPOLYGON M (((0 0 1,1 0 1,0 1 1,0 0 1)),EMPTY)"
        );
    }

    #[test]
    fn geometry_collection_tags_its_members() {
        let mut w = W::new(true);
        w.header(7 | EWKB_SRID).u32(4326).u32(3);
        w.header(1).f(&[1.0, 2.0]);
        w.header(2).u32(2).f(&[0.0, 0.0, 1.0, 1.0]);
        w.header(7).u32(0);
        assert_eq!(
            wkt(&w.bytes()),
            "SRID=4326;GEOMETRYCOLLECTION(POINT(1 2),LINESTRING(0 0,1 1),GEOMETRYCOLLECTION EMPTY)"
        );
        let mut z = W::new(true);
        z.header(1007).u32(1);
        z.header(1001).f(&[1.0, 2.0, 3.0]);
        assert_eq!(wkt(&z.bytes()), "GEOMETRYCOLLECTION Z (POINT Z (1 2 3))");
    }

    #[test]
    fn curves_surfaces_and_tins() {
        let arc = |w: &mut W| {
            w.header(8).u32(3).f(&[0.0, 0.0, 1.0, 1.0, 2.0, 0.0]);
        };
        let mut c = W::new(true);
        arc(&mut c);
        assert_eq!(wkt(&c.bytes()), "CIRCULARSTRING(0 0,1 1,2 0)");

        let mut cc = W::new(true);
        cc.header(9).u32(2);
        arc(&mut cc);
        cc.header(2).u32(2).f(&[2.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            wkt(&cc.bytes()),
            "COMPOUNDCURVE(CIRCULARSTRING(0 0,1 1,2 0),(2 0,0 0))"
        );

        let mut cp = W::new(true);
        cp.header(10).u32(1);
        cp.header(8)
            .u32(5)
            .f(&[0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0, 0.0, 0.0]);
        assert_eq!(
            wkt(&cp.bytes()),
            "CURVEPOLYGON(CIRCULARSTRING(0 0,2 0,2 2,0 2,0 0))"
        );

        let mut mc = W::new(true);
        mc.header(11).u32(2);
        mc.header(2).u32(2).f(&[0.0, 0.0, 5.0, 5.0]);
        arc(&mut mc);
        assert_eq!(
            wkt(&mc.bytes()),
            "MULTICURVE((0 0,5 5),CIRCULARSTRING(0 0,1 1,2 0))"
        );

        let mut ms = W::new(true);
        ms.header(12).u32(1);
        ms.header(3)
            .u32(1)
            .u32(4)
            .f(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(wkt(&ms.bytes()), "MULTISURFACE(((0 0,1 0,0 1,0 0)))");

        let tri = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut ps = W::new(true);
        ps.header(15 | EWKB_Z).u32(1);
        ps.header(3 | EWKB_Z).u32(1).u32(4).f(&tri);
        assert_eq!(
            wkt(&ps.bytes()),
            "POLYHEDRALSURFACE Z (((0 0 0,0 1 0,1 0 0,0 0 0)))"
        );

        let mut tin = W::new(true);
        tin.header(1016).u32(1);
        tin.header(1017).u32(1).u32(4).f(&tri);
        assert_eq!(wkt(&tin.bytes()), "TIN Z (((0 0 0,0 1 0,1 0 0,0 0 0)))");

        let mut t = W::new(true);
        t.header(17)
            .u32(1)
            .u32(4)
            .f(&[0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(wkt(&t.bytes()), "TRIANGLE((0 0,0 1,1 0,0 0))");
    }

    #[test]
    fn mysql_internal_format() {
        // SELECT HEX(ST_GeomFromText('POINT(1 2)', 4326)) — SRID first (little-endian), then WKB.
        let mut bytes = 4326u32.to_le_bytes().to_vec();
        bytes.extend(W::new(true).header(1).f(&[1.0, 2.0]).bytes());
        assert_eq!(
            mysql_to_ewkt(&bytes).as_deref(),
            Some("SRID=4326;POINT(1 2)")
        );
        let mut plain = 0u32.to_le_bytes().to_vec();
        let mut poly = W::new(true);
        poly.header(3)
            .u32(1)
            .u32(4)
            .f(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
        plain.extend(poly.bytes());
        assert_eq!(
            mysql_to_ewkt(&plain).as_deref(),
            Some("POLYGON((0 0,1 0,1 1,0 0))")
        );
        assert_eq!(mysql_to_ewkt(&[0, 0xAB]), None);
    }

    #[test]
    fn numbers_read_back_exactly() {
        let vals = [0.1, -123.456789012345, 1e-7, 2.5e20, 1e16, -0.0, 6378137.0];
        let p = W::new(true).header(2).u32(3).f(&vals[..6]).bytes();
        assert_eq!(
            wkt(&p),
            "LINESTRING(0.1 -123.456789012345,1e-7 2.5e20,10000000000000000 0)"
        );
        for v in vals {
            let mut s = String::new();
            write_number(&mut s, v);
            assert_eq!(s.parse::<f64>().unwrap(), v, "{s}");
        }
    }

    #[test]
    fn malformed_values_are_refused() {
        assert_eq!(parse_wkb(&[]), Err(GeometryError::Truncated));
        assert_eq!(
            parse_wkb(&[2, 1, 0, 0, 0]),
            Err(GeometryError::ByteOrder(2))
        );
        let unknown = W::new(true).header(99).bytes();
        assert_eq!(parse_wkb(&unknown), Err(GeometryError::UnknownType(99)));
        let short = W::new(true).header(1).f(&[1.0]).bytes();
        assert_eq!(parse_wkb(&short), Err(GeometryError::Truncated));
        // A count far beyond the bytes left is refused before allocating.
        let huge = W::new(true).header(2).u32(u32::MAX).bytes();
        assert_eq!(parse_wkb(&huge), Err(GeometryError::Truncated));
        let mut trailing = W::new(true).header(1).f(&[1.0, 2.0]).bytes();
        trailing.push(0);
        assert_eq!(parse_wkb(&trailing), Err(GeometryError::TrailingBytes));
        // Collections nested past the limit.
        let mut deep = W::new(true);
        for _ in 0..=MAX_DEPTH + 1 {
            deep.header(7).u32(1);
        }
        deep.header(1).f(&[0.0, 0.0]);
        assert_eq!(parse_wkb(&deep.bytes()), Err(GeometryError::TooDeep));
        // Not hex at all: left to the caller.
        assert_eq!(ewkb_hex_to_ewkt("POINT(1 2)"), None);
        assert_eq!(ewkb_hex_to_ewkt("0101"), None);
    }

    #[test]
    fn hex_round_trip_of_the_test_writer() {
        let p = W::new(true)
            .header(1 | EWKB_SRID)
            .u32(4326)
            .f(&[1.0, 2.0])
            .bytes();
        assert_eq!(
            hex(&p),
            "0101000020E6100000000000000000F03F0000000000000040"
        );
    }

    #[test]
    fn conversion_flag_defaults_on() {
        assert!(convert_to_wkt());
    }
}
