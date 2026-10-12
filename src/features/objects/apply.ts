import type { QueryClient } from "@tanstack/react-query";
import { guardExecution } from "@/features/safety/guard";
import { errorInfo, payloadInfo, type DdlErrorInfo } from "@/features/structure/previewState";
import { ipc, type DdlPlan, type ObjectOp } from "@/lib/ipc";

export const objectPreviewKey = (connectionId: string, op: ObjectOp | null) =>
  ["objectPreview", connectionId, op] as const;

/** Plans `op`, asks the connection's safe mode about its statements, then runs it. */
export async function guardedApply(
  qc: QueryClient,
  connectionId: string,
  op: ObjectOp,
): Promise<
  | { status: "done"; plan: DdlPlan }
  | { status: "cancelled" }
  | { status: "failed"; error: DdlErrorInfo }
> {
  try {
    const plan = await qc.fetchQuery({
      queryKey: objectPreviewKey(connectionId, op),
      queryFn: () => ipc.objects.preview(connectionId, op),
      staleTime: Infinity,
    });
    if (!(await guardExecution(connectionId, { kind: "commit", statements: plan.statements })))
      return { status: "cancelled" };
    const r = await ipc.objects.apply(connectionId, op);
    if (r.error) return { status: "failed", error: payloadInfo(r.error) };
    return { status: "done", plan };
  } catch (e) {
    return { status: "failed", error: errorInfo(e) };
  }
}
