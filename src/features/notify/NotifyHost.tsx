import { DonePopup } from "./DonePopup";

/** Mounts the "open containing folder" popup of exports and backups. Mount once near the root. */
export function NotifyHost() {
  return <DonePopup />;
}
