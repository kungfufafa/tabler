import { create } from "zustand";
import { ipc, type LocationsInfo } from "@/lib/ipc";

/** The folders in use (Settings ▸ Locations), loaded once and replaced after each change. */
export const useLocationsStore = create<{ info: LocationsInfo | null }>()(() => ({ info: null }));

let inflight: Promise<LocationsInfo> | null = null;

/** Reads the folders from the backend (concurrent calls share one request). */
export function loadLocations(): Promise<LocationsInfo> {
  inflight ??= ipc.locations
    .get()
    .then((info) => {
      useLocationsStore.setState({ info });
      return info;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

export function setLocations(info: LocationsInfo): void {
  useLocationsStore.setState({ info });
}

/** Test helper. */
export function resetLocationsStore(): void {
  inflight = null;
  useLocationsStore.setState({ info: null });
}
