import { Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { DumpTool } from "@/lib/ipc";
import { addFlag, flagCatalog, flagProblem, removeFlag } from "./options";
import { Chip } from "./parts";

/**
 * The option chips of the chosen binary plus "Add option…": a list of common flags; any flag can
 * be typed (`--skip-definer`, `--exclude-table=logs`) and added with ↩.
 */
export function OptionChips({
  tool,
  flags,
  onChange,
  disabled,
}: {
  tool: DumpTool;
  flags: readonly string[];
  onChange: (flags: string[]) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  const catalog = flagCatalog(tool).filter((f) => !flags.includes(f.flag));
  const free = typed.trim().replace(/\s+/g, " ");
  const problem = flagProblem(free);
  const known = flagCatalog(tool).some((f) => f.flag === free);
  const add = (flag: string) => {
    onChange(addFlag(flags, flag));
    setTyped("");
    setOpen(false);
  };
  return (
    <div className="flex flex-wrap items-center gap-1">
      {flags.map((f) => (
        <Chip key={f} disabled={disabled} onRemove={() => onChange(removeFlag(flags, f))}>
          {f}
        </Chip>
      ))}
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setTyped("");
        }}
      >
        <PopoverTrigger asChild>
          <Button type="button" variant="outline" size="xs" disabled={disabled}>
            <Plus /> Add option…
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-80 p-0">
          <Command
            // The typed text is matched by cmdk; a flag that is not listed is added as typed.
            filter={(value, search) => (value.toLowerCase().includes(search.toLowerCase()) ? 1 : 0)}
          >
            <CommandInput
              value={typed}
              onValueChange={setTyped}
              placeholder={`Type a ${tool} option…`}
              className="font-mono text-xs"
              aria-label="Option"
            />
            <CommandList>
              <CommandEmpty className="text-muted-foreground px-3 py-3 text-left text-xs">
                {problem ?? "Type an option and press ↩ to add it."}
              </CommandEmpty>
              {free !== "" && !problem && !known && !flags.includes(free) && (
                <CommandGroup>
                  <CommandItem value={free} onSelect={() => add(free)} className="text-xs">
                    <Plus />
                    Add <span className="font-mono">{free}</span>
                  </CommandItem>
                </CommandGroup>
              )}
              {catalog.length > 0 && (
                <CommandGroup heading={`${tool} options`}>
                  {catalog.map((f) => (
                    <CommandItem
                      key={f.flag}
                      value={`${f.flag} ${f.description}`}
                      onSelect={() => add(f.flag)}
                      className="flex-col items-start gap-0 text-xs"
                    >
                      <span className="font-mono">{f.flag}</span>
                      <span className="text-muted-foreground text-[11px]">{f.description}</span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              )}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </div>
  );
}
