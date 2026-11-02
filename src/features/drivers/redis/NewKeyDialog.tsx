import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import type { RedisNewKeyArgs } from "@/lib/ipc/bindings/RedisNewKeyArgs";
import { NEW_KEY_TYPES, newValueOf, VALUE_HINT } from "./newKey";

/**
 * New key…: its name, type, first value and TTL, as the `newKey` call takes them (refused by the
 * backend when the name is taken); `done` gets the call's arguments, or null when cancelled.
 */
export function NewKeyDialog({
  db,
  done,
}: {
  db: number;
  done: (args: RedisNewKeyArgs | null) => void;
}) {
  const [open, setOpen] = useState(true);
  const [name, setName] = useState("");
  const [type, setType] = useState("string");
  const [value, setValue] = useState("");
  const [ttl, setTtl] = useState("");
  const finish = (args: RedisNewKeyArgs | null) => {
    setOpen(false);
    done(args);
  };
  const parsed = newValueOf(type, value);
  const ttlOk = ttl.trim() === "" || /^\d+$/.test(ttl.trim());
  const problem =
    name === ""
      ? "A new key needs a name"
      : !parsed.ok
        ? parsed.error
        : !ttlOk
          ? "The TTL is a whole number of seconds"
          : null;
  return (
    <Dialog open={open} onOpenChange={(o) => !o && finish(null)}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>New key in db{db}</DialogTitle>
          <DialogDescription>Refused when a key of this name exists.</DialogDescription>
        </DialogHeader>
        <Input
          aria-label="Key name"
          placeholder="user:1"
          className="font-mono"
          value={name}
          onChange={(e) => setName(e.target.value)}
          spellCheck={false}
          autoFocus
        />
        <Select value={type} onValueChange={setType}>
          <SelectTrigger aria-label="Type" className="w-64">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {NEW_KEY_TYPES.map(([id, label]) => (
              <SelectItem key={id} value={id}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Textarea
          aria-label="Value"
          placeholder={VALUE_HINT[type]}
          className="min-h-28 font-mono text-xs"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          spellCheck={false}
        />
        <Input
          aria-label="TTL (seconds)"
          placeholder="TTL in seconds (empty: no expiry)"
          className="w-64 font-mono"
          value={ttl}
          onChange={(e) => setTtl(e.target.value)}
        />
        {problem && name !== "" && <p className="text-destructive text-xs">{problem}</p>}
        <DialogFooter>
          <Button variant="outline" onClick={() => finish(null)}>
            Cancel
          </Button>
          <Button
            disabled={problem !== null}
            onClick={() =>
              parsed.ok &&
              finish({
                db,
                key: { t: "text", v: name },
                ttl: ttl.trim() === "" ? null : { t: "int", v: Number(ttl.trim()) },
                value: parsed.value,
              })
            }
          >
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
