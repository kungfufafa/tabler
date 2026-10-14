import { registerMock } from "./transport";

/**
 * Settings ▸ General ▸ Data Decoder (`set_geometry_wkt`). The mock's spatial columns are generated
 * as EWKT already (see `cellFor` in rows.ts), so it only remembers the setting.
 */
let geometryWkt = true;

registerMock("set_geometry_wkt", ({ enabled }) => {
  geometryWkt = enabled !== false;
  return null;
});

/** Whether the mock backend was told to decode spatial values to WKT. */
export function mockGeometryWkt(): boolean {
  return geometryWkt;
}
