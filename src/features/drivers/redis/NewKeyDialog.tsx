import { useId, useState } from "react";
import { FormError } from "@/components/FormError";
import { FormField } from "@/components/FormField";
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
  const uid = useId();
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
      <DialogContent className="sm:max-w-[34rem]">
        <DialogHeader>
          <DialogTitle className="text-sm">New Key in db{db}</DialogTitle>
          <DialogDescription className="text-xs">
            Refused when a key of this name exists.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-3">
            <FormField label="Key name" htmlFor={`${uid}-name`}>
              <Input
                id={`${uid}-name`}
                placeholder="user:1"
                className="h-7 rounded-md font-mono text-xs md:text-xs"
                value={name}
                onChange={(e) => setName(e.target.value)}
                spellCheck={false}
                autoFocus
              />
            </FormField>
            <FormField label="Type" htmlFor={`${uid}-type`}>
              <Select value={type} onValueChange={setType}>
                <SelectTrigger id={`${uid}-type`} size="sm" className="w-full text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {NEW_KEY_TYPES.map(([id, label]) => (
                    <SelectItem key={id} value={id} className="text-xs">
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </FormField>
          </div>
          <FormField label="Value" htmlFor={`${uid}-value`}>
            <Textarea
              id={`${uid}-value`}
              placeholder={VALUE_HINT[type]}
              className="min-h-28 rounded-md font-mono text-xs md:text-xs"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              spellCheck={false}
            />
          </FormField>
          <div className="grid grid-cols-2 gap-3">
            <FormField
              label="TTL (seconds)"
              htmlFor={`${uid}-ttl`}
              hint="Empty: the key never expires"
            >
              <Input
                id={`${uid}-ttl`}
                placeholder="Optional"
                aria-describedby={`${uid}-ttl-hint`}
                className="h-7 rounded-md font-mono text-xs md:text-xs"
                value={ttl}
                onChange={(e) => setTtl(e.target.value)}
              />
            </FormField>
          </div>
          {problem && name !== "" && <FormError>{problem}</FormError>}
        </div>
        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => finish(null)}>
            Cancel
          </Button>
          <Button
            size="sm"
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
