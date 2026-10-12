import { RotateCcw } from "lucide-react";
import { useRef } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DEFAULT_PATTERN, insertToken, PATTERN_TOKENS, type PatternToken } from "./pattern";
import { Field } from "./parts";

/**
 * "File name": the pattern with its tokens (inserted at the caret) and a live preview of the
 * name the backup gets. A `/` makes sub-folders of the chosen folder.
 */
export function PatternField({
  pattern,
  preview,
  onChange,
  disabled,
}: {
  pattern: string;
  /** The rendered file name (with extension). */
  preview: string;
  onChange: (pattern: string) => void;
  disabled?: boolean;
}) {
  const input = useRef<HTMLInputElement>(null);
  const insert = (token: PatternToken) => {
    const el = input.current;
    const { pattern: next, caret } = insertToken(
      pattern,
      token,
      el?.selectionStart ?? pattern.length,
      el?.selectionEnd ?? pattern.length,
    );
    onChange(next);
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(caret, caret);
    });
  };
  return (
    <Field
      label="File name"
      htmlFor="backup-pattern"
      hint={
        <span className="flex min-w-0 items-baseline gap-1">
          <span className="shrink-0">Preview:</span>
          <span
            data-testid="backup-file-preview"
            className="text-foreground min-w-0 font-mono break-all"
          >
            {preview}
          </span>
        </span>
      }
    >
      <div className="flex items-center gap-1">
        <Input
          ref={input}
          id="backup-pattern"
          value={pattern}
          onChange={(e) => onChange(e.target.value)}
          disabled={disabled}
          spellCheck={false}
          autoComplete="off"
          className="h-7 min-w-0 flex-1 font-mono text-xs"
        />
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Reset file name"
          title="Reset file name"
          disabled={disabled || pattern === DEFAULT_PATTERN}
          onClick={() => onChange(DEFAULT_PATTERN)}
        >
          <RotateCcw />
        </Button>
      </div>
      <div className="flex flex-wrap gap-1" role="group" aria-label="Insert into the file name">
        {PATTERN_TOKENS.map((t) => (
          <button
            key={t.token}
            type="button"
            disabled={disabled}
            onClick={() => insert(t.token)}
            className="border-border text-muted-foreground hover:text-foreground hover:bg-accent h-5 rounded border px-1.5 text-[10px] disabled:opacity-50"
          >
            {t.label}
          </button>
        ))}
      </div>
    </Field>
  );
}
