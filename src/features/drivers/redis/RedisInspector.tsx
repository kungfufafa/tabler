import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { guardExecution } from "@/features/safety/guard";
import { ipc, type Value } from "@/lib/ipc";
import type { RedisEdit } from "@/lib/ipc/bindings/RedisEdit";
import type { RedisEditResult } from "@/lib/ipc/bindings/RedisEditResult";
import type { RedisElement } from "@/lib/ipc/bindings/RedisElement";
import type { RedisKeyPage } from "@/lib/ipc/bindings/RedisKeyPage";
import { errorWithHint as errorText } from "@/lib/errorText";
import { valueText } from "@/lib/values";
import type { InspectorSlotProps } from "../types";
import { ask } from "./ask";
import { dbOf, editableText, TYPE_JSON } from "./keys";

/**
 * The selected key, by its type (TablePlus's right panel, without its traps): a string or a
 * RedisJSON document as text, staged like a grid edit; the elements of a hash, list, set, sorted
 * set or stream a page at a time (`keyPage`, a read), edited one element at a time through
 * `editKey` (each behind the key's type or its own checks, Safe Mode first with the commands
 * `previewEdits` shows); the TTL, staged like a grid edit. Without `onStage` (a read-only
 * connection) everything is read-only. A module's type shows without its value.
 */

const CONTAINERS = new Set(["hash", "list", "set", "zset", "stream"]);
const PAGE = 100;

const show = (v: Value | null | undefined): string => {
  if (!v) return "";
  if (v.t === "bytes")
    return `${v.v.len.toLocaleString()} bytes of binary${v.v.truncated ? " (preview)" : ""}`;
  return valueText(v) ?? "NULL";
};

const text = (v: string): Value => ({ t: "text", v });

/** Copy to…: `COPY` to a new name (6.2+; refused when the name is taken), behind Safe Mode. */
async function copyKey(connectionId: string, db: number, from: Value): Promise<void> {
  const to = await ask({
    title: `Copy ${show(from)}`,
    description: "A copy of the key and its TTL, under a name no key has yet.",
    input: { label: "New name", initial: `${show(from)}:copy` },
    confirmLabel: "Copy",
  });
  if (to === null) return;
  const args = { db, from, to: text(to) };
  const label = `Copy ${show(from)} to ${to}`;
  if (!(await guardExecution(connectionId, { kind: "operation", label, write: true }))) return;
  try {
    const r = await ipc.sessionCall<RedisEditResult>(connectionId, "copyKey", args);
    const failed = r.results.find((x) => !x.ok);
    if (failed) toast.error(failed.error ?? "The key was not copied");
    else toast.success(`Copied to ${to}: reload the page to see it`);
  } catch (e) {
    toast.error(errorText(e));
  }
}

export function RedisInspector({ connectionId, table, row, onStage }: InspectorSlotProps) {
  if (!row)
    return (
      <p className="text-muted-foreground p-3 text-xs" data-testid="redis-no-key">
        Select a key to see its value.
      </p>
    );
  const key = row.key ?? text("");
  const typeCell = row.type;
  const type = typeCell && (typeCell.t === "text" || typeCell.t === "enum") ? typeCell.v : "";
  return (
    <div className="flex flex-col gap-3 p-2 text-xs">
      <div className="flex items-center gap-2">
        <span className="font-mono break-all" data-testid="redis-key">
          {show(key)}
        </span>
        <span className="bg-muted rounded px-1.5 py-0.5 font-mono">{type || "?"}</span>
        {onStage && (
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-6"
            onClick={() => void copyKey(connectionId, dbOf(table), key)}
          >
            Copy to…
          </Button>
        )}
      </div>
      <TtlEditor key={`ttl:${show(key)}`} ttl={row.ttl} onStage={onStage} />
      {type === "string" || type === TYPE_JSON ? (
        <ValueEditor
          key={`v:${show(key)}`}
          value={row.value}
          json={type === TYPE_JSON}
          onStage={onStage}
        />
      ) : CONTAINERS.has(type) ? (
        <Elements
          key={`e:${show(key)}:${type}`}
          connectionId={connectionId}
          db={dbOf(table)}
          redisKey={key}
          type={type}
          editable={onStage !== undefined}
        />
      ) : (
        <p className="text-muted-foreground" data-testid="redis-module-type">
          A {type || "key"} of a module: Tabler shows it without its value (use the console).
        </p>
      )}
    </div>
  );
}

function TtlEditor({
  ttl,
  onStage,
}: {
  ttl: Value | undefined;
  onStage?: InspectorSlotProps["onStage"];
}) {
  const initial = ttl && ttl.t !== "null" ? (valueText(ttl) ?? "") : "";
  const [draft, setDraft] = useState(initial);
  return (
    <div className="flex items-center gap-2">
      <label className="text-muted-foreground w-10" htmlFor="redis-ttl">
        TTL
      </label>
      <Input
        id="redis-ttl"
        aria-label="TTL"
        className="h-7 w-32 font-mono text-xs"
        value={draft}
        placeholder="no expiry"
        disabled={!onStage}
        onChange={(e) => setDraft(e.target.value)}
      />
      <span className="text-muted-foreground">seconds</span>
      {onStage && (
        <Button
          size="sm"
          variant="outline"
          className="h-7"
          disabled={draft.trim() === initial}
          // Empty is no expiry (PERSIST); the backend refuses zero and other negatives.
          onClick={() => onStage({ ttl: draft.trim() === "" ? { t: "null" } : text(draft.trim()) })}
        >
          Stage TTL
        </Button>
      )}
    </div>
  );
}

function ValueEditor({
  value,
  json,
  onStage,
}: {
  value: Value | undefined;
  json: boolean;
  onStage?: InspectorSlotProps["onStage"];
}) {
  const initial = editableText(value);
  const [draft, setDraft] = useState(initial ?? "");
  if (initial === null)
    return (
      <p className="text-muted-foreground" data-testid="redis-preview-only">
        {show(value)}: only a preview is loaded. Load the whole value (the cell's Load value) to
        edit it.
      </p>
    );
  let bad: string | null = null;
  if (json)
    try {
      JSON.parse(draft);
    } catch (e) {
      bad = (e as Error).message;
    }
  return (
    <div className="flex flex-col gap-2">
      <Textarea
        aria-label="Value"
        className="min-h-40 font-mono text-xs"
        spellCheck={false}
        value={draft}
        readOnly={!onStage}
        onChange={(e) => setDraft(e.target.value)}
      />
      {bad && <p className="text-destructive">Not JSON: {bad}</p>}
      {onStage && (
        <Button
          size="sm"
          className="self-start"
          disabled={draft === initial || bad !== null}
          onClick={() => onStage({ value: text(draft) })}
        >
          Stage value
        </Button>
      )}
    </div>
  );
}

interface ElementsProps {
  connectionId: string;
  db: number;
  redisKey: Value;
  type: string;
  editable: boolean;
}

/** The labels of a type's element columns and of its add row. */
const COLUMNS: Record<string, [string, string | null]> = {
  hash: ["Field", "Value"],
  list: ["Index", "Element"],
  set: ["Member", null],
  zset: ["Member", "Score"],
  stream: ["Entry", "Fields"],
};

function Elements({ connectionId, db, redisKey, type, editable }: ElementsProps) {
  const [elements, setElements] = useState<RedisElement[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [size, setSize] = useState<Value | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const target = { db, key: redisKey, type };
  // The key is a value object: its JSON text names it for the hooks.
  const keyText = JSON.stringify(redisKey);

  const load = useCallback(
    async (cursor: string | null) => {
      setBusy(true);
      try {
        // A read: sent as it is, also on a read-only connection.
        const page = await ipc.sessionCall<RedisKeyPage>(connectionId, "keyPage", {
          db,
          key: JSON.parse(keyText) as Value,
          type,
          cursor,
          count: PAGE,
        });
        setElements((old) => (cursor === null ? page.elements : [...old, ...page.elements]));
        setNext(page.nextCursor);
        setSize(page.size);
        setError(null);
      } catch (e) {
        setError(errorText(e));
      } finally {
        setBusy(false);
      }
    },
    [connectionId, db, type, keyText],
  );

  useEffect(() => {
    void load(null);
  }, [load]);

  /** One edit: Safe Mode with its commands, then `editKey`, then the first page again. */
  const edit = async (e: RedisEdit) => {
    const args = { ...target, edits: [e] };
    try {
      const preview = await ipc.sessionCall<string[]>(connectionId, "previewEdits", args);
      if (!(await guardExecution(connectionId, { kind: "commit", statements: preview }))) return;
      const r = await ipc.sessionCall<RedisEditResult>(connectionId, "editKey", args);
      const failed = r.results.find((x) => !x.ok);
      if (failed) toast.error(failed.error ?? "The edit was not applied");
      else {
        setName("");
        setValue("");
      }
    } catch (err) {
      toast.error(errorText(err));
    }
    await load(null);
  };

  const remove = (el: RedisElement): RedisEdit | null => {
    switch (type) {
      case "hash":
        return { op: "hashDelete", field: el.name };
      case "list":
        return el.name.t === "int" && el.value
          ? { op: "listDelete", index: el.name.v, old: el.value }
          : null;
      case "set":
        return { op: "setRemove", member: el.name };
      case "zset":
        return { op: "zsetRemove", member: el.name };
      case "stream":
        return el.name.t === "text" ? { op: "streamDelete", id: el.name.v } : null;
      default:
        return null;
    }
  };

  /** The add row's edit: a field set, an element pushed, a member added or scored, an entry. */
  const added = (): RedisEdit | null => {
    if (name === "" && type !== "list") return null;
    switch (type) {
      case "hash":
        return { op: "hashSet", field: text(name), value: text(value) };
      case "list":
        return { op: "listPush", values: [text(name)], head: false };
      case "set":
        return { op: "setAdd", member: text(name) };
      case "zset": {
        const known = elements.some((el) => el.name.t === "text" && el.name.v === name);
        return known
          ? { op: "zsetScore", member: text(name), score: value }
          : { op: "zsetAdd", member: text(name), score: value };
      }
      case "stream":
        return { op: "streamAdd", id: null, fields: [{ name: text(name), value: text(value) }] };
      default:
        return null;
    }
  };

  const [first, second] = COLUMNS[type] ?? ["Name", "Value"];
  const addLabel: Record<string, string> = {
    hash: "Set field",
    list: "Push",
    set: "Add member",
    zset: "Set score",
    stream: "Add entry",
  };
  const nameLabel = type === "list" ? "Element" : type === "stream" ? "Field" : first;
  const valueLabel = type === "stream" ? "Value" : second;
  return (
    <div className="flex flex-col gap-2" data-testid="redis-elements">
      <div className="text-muted-foreground">
        {size && size.t !== "null" ? `${show(size)} elements` : ""}
        {elements.length > 0 ? `, ${elements.length.toLocaleString()} loaded` : ""}
      </div>
      {error && <p className="text-destructive">{error}</p>}
      <table className="w-full table-fixed border-collapse font-mono">
        <thead>
          <tr className="text-muted-foreground text-left">
            <th className="w-2/5 font-normal">{first}</th>
            {second && <th className="font-normal">{second}</th>}
            {editable && <th className="w-8" />}
          </tr>
        </thead>
        <tbody>
          {elements.map((el, i) => {
            const del = editable ? remove(el) : null;
            return (
              <tr key={i} className="border-t align-top">
                <td className="py-0.5 pr-2 break-all">{show(el.name)}</td>
                {second && <td className="py-0.5 pr-2 break-all">{show(el.value)}</td>}
                {editable && (
                  <td>
                    {del && (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="h-6 px-1"
                        aria-label={`Delete ${show(el.name)}`}
                        disabled={busy}
                        onClick={() => void edit(del)}
                      >
                        ×
                      </Button>
                    )}
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
      {next !== null && (
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          disabled={busy}
          onClick={() => void load(next)}
        >
          Load more
        </Button>
      )}
      {editable && (
        <div className="flex flex-wrap items-center gap-2 border-t pt-2">
          <Input
            aria-label={nameLabel}
            placeholder={nameLabel}
            className="h-7 w-40 font-mono text-xs"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          {valueLabel && (
            <Input
              aria-label={valueLabel}
              placeholder={valueLabel}
              className="h-7 w-40 font-mono text-xs"
              value={value}
              onChange={(e) => setValue(e.target.value)}
            />
          )}
          <Button
            size="sm"
            className="h-7"
            disabled={busy || added() === null}
            onClick={() => {
              const e = added();
              if (e) void edit(e);
            }}
          >
            {addLabel[type] ?? "Add"}
          </Button>
        </div>
      )}
    </div>
  );
}
