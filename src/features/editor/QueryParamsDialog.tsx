import { useState, type KeyboardEvent } from "react";
import { Kbd } from "@/components/Kbd";
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
import { useParamsPromptStore, type ParamsRequest } from "./paramsPrompt";

function ParamsForm({ request }: { request: ParamsRequest }) {
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(request.fields.map((f) => [f.key, request.initial[f.key] ?? ""])),
  );
  const apply = () => request.resolve(values);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.stopPropagation();
      apply();
    }
  };
  const onFieldKeyDown = (e: KeyboardEvent<HTMLInputElement>, index: number) => {
    if (e.key !== "Enter" || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
    e.preventDefault();
    // ↩ moves on to the next value; on the last one it runs.
    const next = e.currentTarget.form?.querySelectorAll("input")[index + 1];
    if (next) next.focus();
    else apply();
  };

  return (
    <DialogContent className="gap-3 sm:max-w-md" onKeyDown={onKeyDown}>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-sm">Query Parameters</DialogTitle>
        <DialogDescription className="text-xs">
          Values go into the SQL exactly as typed: quote text yourself (
          <span className="font-mono">'abc'</span>).
        </DialogDescription>
      </DialogHeader>
      <form
        className="flex max-h-[50vh] flex-col gap-1.5 overflow-auto"
        onSubmit={(e) => {
          e.preventDefault();
          apply();
        }}
      >
        {request.fields.map((f, i) => {
          const id = `query-param-${i}`;
          return (
            <div key={f.key} className="grid grid-cols-[minmax(5rem,auto)_1fr] items-center gap-2">
              <label
                htmlFor={id}
                className="text-muted-foreground truncate text-right font-mono text-xs"
                title={f.label}
              >
                {f.label}
              </label>
              <Input
                id={id}
                autoFocus={i === 0}
                value={values[f.key] ?? ""}
                onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
                onKeyDown={(e) => onFieldKeyDown(e, i)}
                spellCheck={false}
                autoComplete="off"
                className="h-7 font-mono text-xs"
              />
            </div>
          );
        })}
      </form>
      <DialogFooter className="py-3">
        <Button size="sm" variant="outline" onClick={() => request.resolve(null)}>
          Cancel
        </Button>
        <Button size="sm" onClick={apply}>
          Run
          <Kbd
            combo="Mod+Enter"
            className="border-primary-foreground/30 bg-transparent text-inherit"
          />
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

/**
 * The query parameters prompt: one input per distinct variable of the SQL about to run, prefilled
 * with what the tab used last. ⌘↩ (or ↩ in the last field) runs; Esc cancels the run.
 */
export function QueryParamsDialog({ tabId }: { tabId: string }) {
  const request = useParamsPromptStore((s) => (s.request?.tabId === tabId ? s.request : null));
  return (
    <Dialog open={request !== null} onOpenChange={(open) => !open && request?.resolve(null)}>
      {/* A new request (another run) starts from its own values. */}
      {request && <ParamsForm key={request.id} request={request} />}
    </Dialog>
  );
}
