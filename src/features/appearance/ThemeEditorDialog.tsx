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
import { Segmented } from "@/features/settings/controls";
import { baseTokens, type ThemeBase } from "./builtinThemes";
import { cleanThemeName, MAX_THEME_NAME } from "./customTheme";
import { ColorField } from "./fields";
import { ThemePreview } from "./previews";
import type { ThemeDraft } from "./themeActions";
import { THEME_TOKENS, TOKEN_GROUPS, type ThemeToken } from "./tokens";

const BASE_OPTIONS = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
] as const satisfies readonly { value: ThemeBase; label: string }[];

/**
 * The theme editor (New Theme…, Edit…): a name, whether the theme is light or dark, and every
 * colour token as a colour field with a hex input. A small sample window follows the edits live.
 * Save hands back the draft; closing without it changes nothing.
 */
export function ThemeEditorDialog({
  initial,
  title,
  onSave,
  onClose,
}: {
  initial: ThemeDraft;
  title: string;
  onSave: (draft: ThemeDraft) => void;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial.name);
  const [base, setBase] = useState<ThemeBase>(initial.base);
  const [tokens, setTokens] = useState(initial.tokens);
  const cleanName = cleanThemeName(name);
  // Changing the base fills the tokens the theme does not set from the new base.
  const baseColors = baseTokens(base);
  const colorOf = (token: ThemeToken) => tokens[token] ?? baseColors[token];
  const save = () => {
    if (cleanName) onSave({ name: cleanName, base, tokens });
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="flex max-h-[calc(100vh-4rem)] w-[760px] max-w-[calc(100%-2rem)] flex-col gap-3 sm:max-w-[760px]"
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            save();
          }
          // This dialog opens over the Settings dialog, whose ⌘1–⌘9 / ⌘F keys must not fire here.
          e.stopPropagation();
        }}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            Colours are hex values, <code className="font-mono">#rrggbb</code> or{" "}
            <code className="font-mono">#rrggbbaa</code> for tints. The sample below follows your
            changes.
          </DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-3">
          <label htmlFor="theme-name" className="text-xs">
            Name
          </label>
          <Input
            id="theme-name"
            value={name}
            maxLength={MAX_THEME_NAME}
            autoFocus
            spellCheck={false}
            aria-invalid={cleanName ? undefined : true}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                save();
              }
            }}
            className="h-7 flex-1 rounded-md px-2 text-xs"
          />
          <span id="theme-base-label" className="text-xs">
            Based on
          </span>
          <Segmented id="theme-base" value={base} options={BASE_OPTIONS} onChange={setBase} />
        </div>
        <ThemePreview base={base} tokens={tokens} />
        <div className="min-h-0 flex-1 overflow-y-auto pr-1">
          {TOKEN_GROUPS.map((group) => (
            <section key={group} className="mb-3">
              <h3 className="text-muted-foreground mb-1 text-[11px] font-medium">{group}</h3>
              <div className="grid grid-cols-2 gap-x-6">
                {THEME_TOKENS.filter((t) => t.group === group).map((t) => (
                  <ColorField
                    key={t.name}
                    id={`theme-color-${t.name}`}
                    label={t.label}
                    value={colorOf(t.name)}
                    onChange={(hex) => setTokens((prev) => ({ ...prev, [t.name]: hex }))}
                  />
                ))}
              </div>
            </section>
          ))}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={save} disabled={!cleanName}>
            Save Theme
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
