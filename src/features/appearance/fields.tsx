import { RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { isFontFamily, MAX_FONT_FAMILY_LENGTH } from "./appearancePrefs";
import { colorToHex, colorToOpaqueHex, parseColor, toHex } from "./color";
import { normalizeHexInput, parseDecimal } from "./inputs";

/**
 * One colour of a theme: a swatch (the system colour picker), a hex field (`#rrggbb`, or
 * `#rrggbbaa` for translucent tints) and, once the colour is not the theme's own, a reset button.
 * `value` may be hex or `oklch(…)`; changes are reported as hex.
 */
export function ColorField({
  id,
  label,
  value,
  overridden = false,
  onChange,
  onReset,
}: {
  id: string;
  label: string;
  value: string;
  overridden?: boolean;
  onChange: (hex: string) => void;
  onReset?: () => void;
}) {
  const hex = colorToHex(value);
  const [draft, setDraft] = useState<string | null>(null);
  const draftRef = useRef<string | null>(null);
  const update = (next: string | null) => {
    draftRef.current = next;
    setDraft(next);
  };
  const commit = () => {
    const pending = draftRef.current;
    if (pending === null) return;
    update(null);
    const next = normalizeHexInput(pending);
    if (next && next !== hex) onChange(next);
  };
  const alpha = parseColor(value)?.a ?? 1;
  return (
    <div className="flex h-8 items-center gap-2">
      <label id={`${id}-label`} htmlFor={id} className="min-w-0 flex-1 truncate text-xs">
        {label}
      </label>
      <input
        id={`${id}-picker`}
        type="color"
        aria-label={`${label} colour picker`}
        value={colorToOpaqueHex(value)}
        onChange={(e) => {
          // The picker is opaque: a translucent tint keeps its own alpha.
          const picked = parseColor(e.target.value);
          if (picked) onChange(toHex({ ...picked, a: alpha }));
        }}
        className="border-border h-6 w-7 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
      />
      <Input
        id={id}
        value={draft ?? hex}
        spellCheck={false}
        aria-labelledby={`${id}-label`}
        aria-invalid={draft !== null && normalizeHexInput(draft) === null ? true : undefined}
        onChange={(e) => update(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape" && draftRef.current !== null) {
            e.stopPropagation();
            update(null);
          }
        }}
        className="h-7 w-[5.5rem] shrink-0 rounded-md px-2 font-mono text-xs uppercase"
      />
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        title="Reset to theme default"
        aria-label={`Reset ${label} to theme default`}
        onClick={onReset}
        className={cn("shrink-0", !overridden && "pointer-events-none invisible")}
        tabIndex={overridden ? 0 : -1}
      >
        <RotateCcw />
      </Button>
    </div>
  );
}

/**
 * A font-family field: any installed font can be typed (several, comma-separated, work as a
 * fallback list); `suggestions` fill a datalist, led by `defaultLabel` ("System monospace"), which
 * stands for the app's default and is stored as empty. Commits on Enter, blur and unmount; invalid
 * text reverts.
 */
export function FontField({
  id,
  value,
  suggestions,
  defaultLabel,
  onCommit,
}: {
  id: string;
  value: string;
  suggestions: readonly string[];
  /** What the empty value is called: the placeholder, and a suggestion that clears the field. */
  defaultLabel: string;
  onCommit: (value: string) => void;
}) {
  /** The stored text for typed text: the default's name means the default (empty). */
  const stored = (typed: string) => {
    const clean = typed.replace(/\s+/g, " ").trim();
    return clean.toLowerCase() === defaultLabel.toLowerCase() ? "" : clean;
  };
  const [draft, setDraftState] = useState<string | null>(null);
  const draftRef = useRef<string | null>(null);
  const latest = useRef({ onCommit, stored });
  useEffect(() => {
    latest.current = { onCommit, stored };
  });
  const setDraft = (next: string | null) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const commit = () => {
    const pending = draftRef.current;
    if (pending === null) return;
    setDraft(null);
    const clean = stored(pending);
    if (isFontFamily(clean) && clean !== value) onCommit(clean);
  };
  // Closing the dialog with focus in the field removes it without a blur: commit then too.
  useEffect(
    () => () => {
      const pending = draftRef.current;
      draftRef.current = null;
      if (pending === null) return;
      const clean = latest.current.stored(pending);
      if (isFontFamily(clean)) latest.current.onCommit(clean);
    },
    [],
  );
  const shown = draft ?? value;
  return (
    <>
      <Input
        id={id}
        list={`${id}-list`}
        value={shown}
        placeholder={defaultLabel}
        maxLength={MAX_FONT_FAMILY_LENGTH}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-labelledby={`${id}-label`}
        aria-describedby={`${id}-description`}
        aria-invalid={draft !== null && !isFontFamily(draft) ? true : undefined}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        className="h-7 w-48 rounded-md px-2 text-xs"
      />
      <datalist id={`${id}-list`}>
        {[defaultLabel, ...suggestions].map((name) => (
          <option key={name} value={name} />
        ))}
      </datalist>
    </>
  );
}

/** A decimal input (line heights) that commits on Enter, blur and unmount, clamped and snapped to `step`. */
export function DecimalField({
  id,
  value,
  min,
  max,
  step,
  suffix,
  onCommit,
}: {
  id: string;
  value: number;
  min: number;
  max: number;
  step: number;
  suffix?: string;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraftState] = useState<string | null>(null);
  const draftRef = useRef<string | null>(null);
  const latest = useRef({ min, max, step, onCommit });
  useEffect(() => {
    latest.current = { min, max, step, onCommit };
  });
  const setDraft = (next: string | null) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const commit = () => {
    const pending = draftRef.current;
    if (pending === null) return;
    setDraft(null);
    const n = parseDecimal(pending, min, max, step);
    if (n !== null && n !== value) onCommit(n);
  };
  useEffect(
    () => () => {
      const pending = draftRef.current;
      draftRef.current = null;
      if (pending === null) return;
      const l = latest.current;
      const n = parseDecimal(pending, l.min, l.max, l.step);
      if (n !== null) l.onCommit(n);
    },
    [],
  );
  return (
    <div className="flex items-center gap-1.5">
      <Input
        id={id}
        type="number"
        inputMode="decimal"
        min={min}
        max={max}
        step={step}
        value={draft ?? String(value)}
        aria-labelledby={`${id}-label`}
        aria-describedby={`${id}-description`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        className="h-7 w-16 [appearance:textfield] rounded-md px-2 text-right text-xs tabular-nums [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
      />
      {suffix && <span className="text-muted-foreground text-xs">{suffix}</span>}
    </div>
  );
}
