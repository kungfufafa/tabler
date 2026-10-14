import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/features/settings/controls";
import { CSV_DECIMALS, decimalProblem, isCustomDecimal } from "./options";

const mono = (label: string) => <span className="font-mono">{label}</span>;

/**
 * The decimal separator: `.`, `,` or "Other…" with a one-character field (TablePlus's custom
 * decimal delimiter). Used by the export dialog and Settings ▸ General ▸ CSV default options.
 *
 * `commitInvalid`: the export dialog takes whatever is typed (and refuses to export it, with the
 * message below the options); the settings only store a character the export accepts and say why
 * they did not.
 */
export function DecimalChoice({
  id,
  value,
  onChange,
  delimiter,
  commitInvalid = false,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  delimiter: string;
  commitInvalid?: boolean;
}) {
  const [other, setOther] = useState(() => isCustomDecimal(value));
  // What is typed in the field (a character the settings refuse stays here, not in `value`).
  const [text, setText] = useState(() => (isCustomDecimal(value) ? value : ""));
  const problem = other ? decimalProblem(text, delimiter) : null;
  return (
    <div className="flex flex-col items-start gap-1">
      <div className="flex items-center gap-2">
        <Segmented<string>
          id={id}
          value={other ? "other" : value}
          onChange={(v) => {
            if (v === "other") {
              setOther(true);
              // The field starts empty: choosing "Other…" is not a choice of a character yet.
              if (commitInvalid && !isCustomDecimal(value)) {
                setText("");
                onChange("");
              }
              return;
            }
            setOther(false);
            onChange(v);
          }}
          options={[
            ...CSV_DECIMALS.map((d) => ({ ...d, label: mono(d.label) })),
            { value: "other", label: "Other…", title: "Another single character" },
          ]}
        />
        {other && (
          <Input
            aria-label="Custom decimal separator"
            aria-invalid={problem !== null}
            value={text}
            maxLength={1}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => {
              const next = e.target.value;
              setText(next);
              if (commitInvalid || decimalProblem(next, delimiter) === null) onChange(next);
            }}
            className="h-7 w-12 text-center font-mono text-xs"
          />
        )}
      </div>
      {problem && !commitInvalid && (
        <span role="status" className="text-warning text-[11px] leading-4">
          {problem}
        </span>
      )}
    </div>
  );
}
