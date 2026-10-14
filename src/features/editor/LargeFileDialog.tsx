import { useState } from "react";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  answerLargeFile,
  HEAD_BYTES,
  sizeLabel,
  useLargeFileStore,
  type LargeFileRequest,
} from "./largeFile";

/**
 * "This file is big": asked before an SQL file above `LARGE_FILE_BYTES` (`LARGE_GZ_BYTES` when
 * gzipped) is loaded into an editor, which would freeze the app on a multi-hundred-MB dump. The
 * choices are its first 5 MB (an unsaved copy: the file is never overwritten), the whole file,
 * the SQL import wizard (streams the file, nothing is loaded here) or Cancel. Mount once.
 */
export function LargeFileDialog() {
  const live = useLargeFileStore((s) => s.request);
  // Keep the text while the dialog animates out.
  const [last, setLast] = useState<LargeFileRequest | null>(live);
  if (live && live !== last) setLast(live);
  const req = live ?? last;
  return (
    <AlertDialog open={live !== null} onOpenChange={(open) => !open && answerLargeFile("cancel")}>
      <AlertDialogContent className="gap-3 sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm font-semibold">
            {req ? `${req.name} is ${sizeLabel(req.bytes)}` : "Large file"}
          </AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            {req?.compressed
              ? "It unpacks to several times that. "
              : "The editor loads a file whole, and this much text makes it slow to a crawl. "}
            The first {sizeLabel(HEAD_BYTES)} opens as an unsaved copy, so saving it never
            overwrites the file; or run the whole script through the SQL import wizard, which reads
            it in a stream.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="flex-col gap-1.5 px-0 sm:flex-col sm:items-stretch">
          <Button size="sm" onClick={() => answerLargeFile("head")} autoFocus>
            Open the first {sizeLabel(HEAD_BYTES)} (read-only)
          </Button>
          {req?.canImport && (
            <Button size="sm" variant="outline" onClick={() => answerLargeFile("import")}>
              Import as SQL dump…
            </Button>
          )}
          <Button size="sm" variant="outline" onClick={() => answerLargeFile("anyway")}>
            Open anyway
          </Button>
          <Button size="sm" variant="ghost" onClick={() => answerLargeFile("cancel")}>
            Cancel
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
