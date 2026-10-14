import { toast } from "sonner";
import {
  baseName,
  headTitle,
  pickOpenPath,
  pickSavePath,
  readSqlFileGuarded,
} from "@/features/editor/files";
import { trackRecentFile } from "@/features/recent/track";
import { ipc, toIpcError } from "@/lib/ipc";
import { useTabsStore } from "@/stores/tabs";
import { loadViewerText, markViewerSaved, viewerText } from "./buffers";
import { getDoc, patchDoc } from "./store";

/**
 * Reads `path` into the viewer (replacing its buffer). Resolves false when the file could not be
 * read (the viewer shows why) or was not loaded: a big file asks first (`readSqlFileGuarded`) and
 * the user may cancel or send it to the SQL import wizard instead. The head of a big file opens
 * as a copy that is not linked to the file, so saving can never truncate the original.
 */
export async function loadViewerFile(key: string, path: string): Promise<boolean> {
  const doc = getDoc(key);
  if (!doc) return false;
  patchDoc(key, { loading: true, error: null, pendingPath: null });
  let file: Awaited<ReturnType<typeof readSqlFileGuarded>>;
  try {
    // `.sql.gz` files open decompressed. A viewer tab can send a big file to its workspace's
    // import wizard; the standalone viewer has no connection to import into.
    const connectionId = useTabsStore.getState().tabs.find((t) => t.id === key)?.connectionId;
    file = await readSqlFileGuarded(path, { connectionId });
  } catch (e) {
    const message = toIpcError(e).message;
    patchDoc(key, { loading: false, error: message });
    toast.error("Could not open the file", { description: message });
    return false;
  }
  const current = getDoc(key);
  if (!current) return false;
  if (!file) {
    // Cancelled, or handed to the import wizard: the viewer keeps what it had.
    patchDoc(key, { loading: false });
    return false;
  }
  loadViewerText(key, file.text, current.dialect);
  patchDoc(key, {
    path: file.readOnlyHead ? null : path,
    headOf: file.readOnlyHead ? path : null,
    headNotice: file.headNotice,
    loading: false,
    error: null,
    revision: current.revision + 1,
  });
  trackRecentFile(path);
  return true;
}

/** Asks for a `.sql` file to open. */
export function pickViewerFile(): Promise<string | null> {
  return pickOpenPath();
}

/**
 * Saves the buffer to its file, asking for a path when it has none (or always with `saveAs`).
 * Resolves with the path saved to, or null.
 */
export async function saveViewer(
  key: string,
  opts: { saveAs?: boolean } = {},
): Promise<string | null> {
  const doc = getDoc(key);
  if (!doc) return null;
  let path = doc.path;
  if (!path || opts.saveAs) {
    const suggested = path ? baseName(path) : doc.headOf ? headTitle(doc.headOf) : "Untitled.sql";
    path = await pickSavePath(suggested);
  }
  if (!path) return null;
  const text = viewerText(key);
  try {
    // A `.sql.gz` path is written compressed.
    await ipc.sqlFiles.write(path, text);
  } catch (e) {
    toast.error("Could not save the file", { description: toIpcError(e).message });
    return null;
  }
  markViewerSaved(key, text);
  // Saved: it follows its own file now (the head of a big file is a copy no more).
  patchDoc(key, { path, error: null, headOf: null, headNotice: null });
  trackRecentFile(path);
  toast.success(`Saved ${baseName(path)}`);
  return path;
}
