import { toast } from "sonner";
import { KIND_LABEL } from "@/features/structure/tableActions";
import { ipc, toIpcError } from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { useConnectionsStore } from "@/stores/connections";
import {
  opTargets,
  routineName,
  stageObjectOp,
  unstageObjectOp,
  type StagedObjectOp,
} from "./pendingOps";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * What a staged op does, in a toast's words: "drop table users", "drop 2 tables CASCADE",
 * "truncate 3 tables without foreign key checks", "drop function slugify(value text)".
 */
export function stagedSummary(op: StagedObjectOp): string {
  let text: string;
  switch (op.op) {
    case "dropTables": {
      const first = op.tables[0];
      if (op.tables.length === 1 && first)
        text = `drop ${KIND_LABEL[first.kind].toLowerCase()} ${first.table.name}`;
      else {
        const isView = (k: string) => k === "view" || k === "materializedView";
        const noun = op.tables.every((t) => isView(t.kind))
          ? "view"
          : op.tables.every((t) => !isView(t.kind) && t.kind !== "systemView")
            ? "table"
            : "item";
        text = `drop ${plural(op.tables.length, noun)}`;
      }
      break;
    }
    case "truncateTables":
      text =
        op.tables.length === 1 && op.tables[0]
          ? `truncate ${op.tables[0].name}`
          : `truncate ${plural(op.tables.length, "table")}`;
      break;
    case "dropRoutines": {
      const kinds = new Set(
        op.routines.map((r) => (r.kind === "procedure" ? "procedure" : "function")),
      );
      const noun = kinds.size === 1 ? [...kinds][0]! : "routine";
      const first = op.routines[0];
      text =
        op.routines.length === 1 && first
          ? `drop ${noun} ${routineName(first)}`
          : `drop ${plural(op.routines.length, noun)}`;
      break;
    }
    case "dropSequences":
      text =
        op.sequences.length === 1 && op.sequences[0]
          ? `drop sequence ${op.sequences[0].name}`
          : `drop ${plural(op.sequences.length, "sequence")}`;
      break;
  }
  if (op.cascade) text += " CASCADE";
  if ((op.op === "dropTables" || op.op === "truncateTables") && op.ignoreForeignKeys)
    text += " without foreign key checks";
  return text;
}

/** "⌘S to commit, ⇧⌘⌫ to discard" with the user's bindings. */
function commitHint(): string {
  const commit = shortcutFor("grid.commit");
  const discard = shortcutFor("grid.discard");
  return `${commit ? formatCombo(commit) : "Commit"} to commit, ${
    discard ? formatCombo(discard) : "Discard"
  } to discard`;
}

/** Ops whose plan is being fetched, so a repeated key press does not stage them twice. */
const inFlight = new Set<string>();

/**
 * ⌥⌫ / ⇧⌫ / ⌘⌫ on sidebar items (TablePlus: "the shortcut skips the dialog"): previews the op's
 * SQL and stages it right away, like the dialog's confirm button would, then says so in a toast
 * with Undo (takes it out again). Resolves false when nothing was staged (the plan failed).
 */
export async function stageWithoutDialog(
  connectionId: string,
  op: StagedObjectOp,
): Promise<boolean> {
  const what = stagedSummary(op);
  const id = `${connectionId}|${op.op}|${opTargets(op)
    .map((t) => t.key)
    .join(",")}`;
  if (inFlight.has(id)) return false;
  inFlight.add(id);
  try {
    const plan = await ipc.objects.preview(connectionId, op);
    const database =
      useConnectionsStore.getState().byId[connectionId]?.info.currentDatabase ?? null;
    const pending = stageObjectOp(connectionId, op, plan.statements, database);
    toast.message(`Staged: ${what}`, {
      description: [commitHint(), ...plan.warnings].join("\n"),
      action: { label: "Undo", onClick: () => unstageObjectOp(connectionId, pending.id) },
    });
    return true;
  } catch (e) {
    toast.error(`Could not ${what}`, { description: toIpcError(e).message });
    return false;
  } finally {
    inFlight.delete(id);
  }
}
