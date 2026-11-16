import { Minus, Plus } from "lucide-react";
import { useId, useState } from "react";
import { FormError } from "@/components/FormError";
import { FormField } from "@/components/FormField";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/features/admin/parts";
import { cn } from "@/lib/utils";
import {
  closeGroupDialog,
  createGroup,
  editGroup,
  GROUP_KINDS,
  NAME_OPS,
  regexError,
  useGroupDialogStore,
  type GroupDialogRequest,
  type GroupKind,
  type GroupRule,
  type NameOp,
} from "./groups";

type Field = GroupRule["field"];

/** A rule being edited (every field kept, so switching Name ↔ Kind does not lose the value). */
interface RuleDraft {
  field: Field;
  op: NameOp;
  value: string;
  kind: GroupKind;
}

const NEW_RULE: RuleDraft = { field: "name", op: "prefix", value: "", kind: "table" };

function toDraft(rule: GroupRule): RuleDraft {
  switch (rule.field) {
    case "name":
      return { ...NEW_RULE, op: rule.op, value: rule.value };
    case "kind":
      return { ...NEW_RULE, field: "kind", kind: rule.value };
    case "schema":
      return { ...NEW_RULE, field: "schema", value: rule.value };
  }
}

function toRule(d: RuleDraft): GroupRule {
  switch (d.field) {
    case "name":
      return { field: "name", op: d.op, value: d.value };
    case "kind":
      return { field: "kind", value: d.kind };
    case "schema":
      return { field: "schema", value: d.value.trim() };
  }
}

/** Why the rule cannot be saved, or null. */
function ruleError(d: RuleDraft): string | null {
  if (d.field === "kind") return null;
  if (d.value.trim() === "") return "Fill in every rule";
  if (d.field === "name" && d.op === "regex") {
    const err = regexError(d.value);
    return err ? `Invalid regex: ${err}` : null;
  }
  return null;
}

function GroupForm({ request }: { request: GroupDialogRequest }) {
  const id = useId();
  const editing = request.group;
  const [name, setName] = useState(editing?.name ?? "");
  const [useRules, setUseRules] = useState(editing ? editing.rules !== null : false);
  const [rules, setRules] = useState<RuleDraft[]>(
    editing?.rules && editing.rules.length > 0 ? editing.rules.map(toDraft) : [NEW_RULE],
  );
  const setRule = (i: number, patch: Partial<RuleDraft>) =>
    setRules((list) => list.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const errors = [
    ...(name.trim() === "" ? ["Name is required"] : []),
    ...(useRules ? rules.map(ruleError).filter((e): e is string => e !== null) : []),
  ];
  const valid = errors.length === 0;
  const save = () => {
    if (!valid) return;
    const saved = useRules ? rules.map(toRule) : null;
    if (editing) editGroup(request.scope, editing.id, name, saved);
    else createGroup(request.scope, name, saved, request.members ?? []);
    closeGroupDialog();
  };

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <FormField label="Name" htmlFor={`${id}-name`}>
        <Input
          id={`${id}-name`}
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Partitions"
          className="h-7 rounded-md text-xs md:text-xs"
          spellCheck={false}
          autoComplete="off"
        />
      </FormField>
      <label className="flex h-7 items-center gap-2 text-xs">
        <Checkbox checked={useRules} onCheckedChange={(v) => setUseRules(v === true)} />
        Use filter rules
      </label>
      {useRules ? (
        <div className="flex flex-col gap-1.5" role="group" aria-label="Filter rules">
          <p className="text-muted-foreground text-xs leading-4">
            Items matching all of these rules are listed in the group.
          </p>
          {rules.map((r, i) => (
            <div key={i} className="grid grid-cols-[6.5rem_8rem_1fr_auto] items-center gap-1.5">
              <NativeSelect
                aria-label={`Rule ${i + 1} field`}
                value={r.field}
                onChange={(e) => setRule(i, { field: e.target.value as Field })}
              >
                <option value="name">Name</option>
                <option value="kind">Kind</option>
                <option value="schema">Schema</option>
              </NativeSelect>
              {r.field === "name" ? (
                <NativeSelect
                  aria-label={`Rule ${i + 1} operator`}
                  value={r.op}
                  onChange={(e) => setRule(i, { op: e.target.value as NameOp })}
                >
                  {NAME_OPS.map(([op, label]) => (
                    <option key={op} value={op}>
                      {label}
                    </option>
                  ))}
                </NativeSelect>
              ) : (
                <span className="text-muted-foreground px-2 text-xs">is</span>
              )}
              {r.field === "kind" ? (
                <NativeSelect
                  aria-label={`Rule ${i + 1} kind`}
                  value={r.kind}
                  onChange={(e) => setRule(i, { kind: e.target.value as GroupKind })}
                >
                  {GROUP_KINDS.map(([kind, label]) => (
                    <option key={kind} value={kind}>
                      {label}
                    </option>
                  ))}
                </NativeSelect>
              ) : (
                <Input
                  aria-label={`Rule ${i + 1} value`}
                  value={r.value}
                  onChange={(e) => setRule(i, { value: e.target.value })}
                  placeholder={
                    r.field === "schema" ? "public" : r.op === "regex" ? "^log_\\d+$" : ""
                  }
                  aria-invalid={ruleError(r) !== null && r.value !== "" ? true : undefined}
                  className={cn(
                    "h-7 rounded-md font-mono text-xs md:text-xs",
                    ruleError(r) !== null && r.value !== "" && "border-error",
                  )}
                  spellCheck={false}
                  autoComplete="off"
                />
              )}
              <div className="flex gap-0.5">
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Remove rule ${i + 1}`}
                  disabled={rules.length === 1}
                  onClick={() => setRules((list) => list.filter((_, j) => j !== i))}
                >
                  <Minus />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`Add a rule after rule ${i + 1}`}
                  onClick={() =>
                    setRules((list) => [...list.slice(0, i + 1), NEW_RULE, ...list.slice(i + 1)])
                  }
                >
                  <Plus />
                </Button>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-muted-foreground text-xs leading-4">
          Drag tables, views or functions onto the group (or use Add to Group in their menu).
        </p>
      )}
      {!valid && name !== "" && <FormError>{errors[0]}</FormError>}
      <DialogFooter>
        <Button type="button" variant="outline" size="sm" onClick={closeGroupDialog}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={!valid}>
          {editing ? "Save" : "Create"}
        </Button>
      </DialogFooter>
    </form>
  );
}

/** New Group… / Edit Group… of the sidebar (`openGroupDialog`). */
export function GroupDialogHost() {
  const request = useGroupDialogStore((s) => s.request);
  if (!request) return null;
  return (
    <Dialog open onOpenChange={(o) => !o && closeGroupDialog()}>
      <DialogContent className="sm:max-w-[34rem]">
        <DialogHeader>
          <DialogTitle className="text-sm">
            {request.group ? "Edit Group" : "New Group"}
          </DialogTitle>
          <DialogDescription className="text-xs">
            Groups are listed above Tables; their items leave the other sections.
          </DialogDescription>
        </DialogHeader>
        <GroupForm key={request.group?.id ?? "new"} request={request} />
      </DialogContent>
    </Dialog>
  );
}
