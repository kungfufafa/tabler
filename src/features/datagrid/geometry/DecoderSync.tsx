import { useEffect } from "react";
import { ipc } from "@/lib/ipc";
import { usePrefs } from "@/stores/prefs";

/**
 * Keeps the backend's spatial decoding in step with Settings ▸ General ▸ Data Decoder
 * ("Automatically convert PostGIS geometry to WKT"): sent at startup and on every change. Rows
 * loaded after a change use the new form (⌘R reloads a table).
 */
export function GeometryDecoderSync() {
  const wkt = usePrefs((p) => p.geometryToWkt);
  useEffect(() => {
    ipc.decoder.setGeometryWkt(wkt).catch((e: unknown) => {
      console.warn("[geometry] could not update the decoder", e);
    });
  }, [wkt]);
  return null;
}
