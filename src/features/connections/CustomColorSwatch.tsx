import { useEffect, useId, useRef, useState } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { isHexColor } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { parseHex } from "./colors";

const RAINBOW =
  "conic-gradient(oklch(0.65 0.2 25), oklch(0.75 0.16 85), oklch(0.65 0.16 150), oklch(0.62 0.13 220), oklch(0.58 0.18 300), oklch(0.65 0.2 25))";

/**
 * The "Custom…" status colour (TablePlus's free colour picker with its Hex field): a swatch after
 * the presets that opens a native colour picker and a Hex field. Shows the custom colour once one
 * is chosen (`value` is a `#rrggbb` colour then; a preset name otherwise).
 */
export function CustomColorSwatch({
  value,
  onChange,
  disabled = false,
  className,
}: {
  value: string | null | undefined;
  onChange: (hex: string) => void;
  disabled?: boolean;
  className?: string;
}) {
  const uid = useId();
  const custom = isHexColor(value) ? value.toLowerCase() : null;
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(custom ?? "");
  // The draft starts from the saved colour each time the popover opens. It is not reset when the
  // colour is saved while the popover is open: the picker fires on every step of a drag, and the
  // saved colour of an earlier step would snap the picker back under the user's pointer.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open && !wasOpen.current) setDraft(custom ?? "");
    wasOpen.current = open;
  }, [open, custom]);
  const parsed = parseHex(draft);
  const commit = () => {
    if (parsed && parsed !== custom) onChange(parsed);
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          role="radio"
          aria-checked={custom !== null}
          aria-label="Custom…"
          title={custom ? `Custom: ${custom}` : "Custom…"}
          disabled={disabled}
          className={cn(
            "ring-offset-background size-4 rounded-full ring-offset-2 transition-transform outline-none",
            "focus-visible:ring-ring focus-visible:ring-2 disabled:opacity-60",
            custom ? "ring-foreground/60 scale-110 ring-2" : "hover:scale-110",
            className,
          )}
          style={{ background: custom ?? RAINBOW }}
        />
      </PopoverTrigger>
      <PopoverContent align="end" className="w-56 gap-2 p-3">
        <div className="flex items-center gap-2">
          <input
            type="color"
            aria-label="Colour picker"
            value={parsed ?? custom ?? "#3b82f6"}
            onChange={(e) => {
              setDraft(e.target.value);
              onChange(e.target.value.toLowerCase());
            }}
            className="size-8 shrink-0 cursor-pointer rounded border-0 bg-transparent p-0"
          />
          <Label htmlFor={`${uid}-hex`} className="text-muted-foreground text-xs font-normal">
            Hex
          </Label>
          <Input
            id={`${uid}-hex`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commit();
                if (parsed) setOpen(false);
              }
            }}
            placeholder="#3b82f6"
            spellCheck={false}
            autoComplete="off"
            className="h-7 min-w-0 flex-1 font-mono text-xs"
            aria-invalid={draft !== "" && !parsed ? true : undefined}
          />
        </div>
      </PopoverContent>
    </Popover>
  );
}
