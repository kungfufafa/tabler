import { create } from "zustand";

/** Plain SQL files above this size ask before they open in an editor (CodeMirror holds them whole). */
export const LARGE_FILE_BYTES = 8 * 1024 * 1024;
/** The same for a gzip file, by its compressed size: it unpacks to several times that. */
export const LARGE_GZ_BYTES = 2 * 1024 * 1024;
/** How much "Open the first 5 MB" reads. */
export const HEAD_BYTES = 5 * 1024 * 1024;

/** What the user chose for a file that is too big to open without asking. */
export type LargeFileChoice = "head" | "anyway" | "import" | "cancel";

export interface LargeFileRequest {
  name: string;
  /** Size on disk. */
  bytes: number;
  compressed: boolean;
  /** "Import as SQL dump…" needs a connection to import into. */
  canImport: boolean;
  resolve: (choice: LargeFileChoice) => void;
}

export const useLargeFileStore = create<{ request: LargeFileRequest | null }>()(() => ({
  request: null,
}));

/**
 * Asks what to do with a big SQL file (`LargeFileDialog` shows the question). A newer question
 * cancels an older one.
 */
export function askLargeFile(info: Omit<LargeFileRequest, "resolve">): Promise<LargeFileChoice> {
  return new Promise((resolve) => {
    useLargeFileStore.getState().request?.resolve("cancel");
    useLargeFileStore.setState({ request: { ...info, resolve } });
  });
}

/** Answers the open question (no-op without one). */
export function answerLargeFile(choice: LargeFileChoice): void {
  const request = useLargeFileStore.getState().request;
  if (!request) return;
  useLargeFileStore.setState({ request: null });
  request.resolve(choice);
}

/** "5 MB", "12.4 MB", "1.20 GB". */
export function sizeLabel(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) return `${(mb / 1024).toFixed(2)} GB`;
  return `${Number.isInteger(mb) ? mb : mb.toFixed(1)} MB`;
}
