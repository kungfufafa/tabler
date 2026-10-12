import { toast } from "sonner";
import { writeClipboard } from "@/features/datagrid/clipboard";

/** Copies `text`, confirming with a toast (or reporting why it failed). */
export function copy(text: string, what = "Copied"): void {
  void writeClipboard(text).then(
    () => toast.success(what),
    (e: unknown) => toast.error("Could not copy", { description: (e as Error).message }),
  );
}
