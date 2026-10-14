//! Settings ▸ General ▸ Data Decoder.

/// "Automatically convert PostGIS geometry to WKT": spatial values decoded from now on are WKT
/// (`SRID=4326;POINT(1 2)`) instead of hex. The renderer sends it at startup and on every change.
#[tauri::command]
pub fn set_geometry_wkt(enabled: bool) {
    crate::geometry::set_convert_to_wkt(enabled);
}
