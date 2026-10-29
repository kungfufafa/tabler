import { useEffect, useId, useRef, useState } from "react";
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
import { loadUrlSchemes, parseConnectionUrl, type ParsedConnectionUrl } from "./url";

/**
 * "New connection from URL…": one field for a `postgres://` / `mysql://` / `mariadb://` /
 * `sqlite://` URL. A valid URL hands its parsed values (password included) to `onParsed`, which
 * opens the connection form prefilled; anything else shows why, inline.
 */
export function ConnectionUrlDialog({
  open,
  onOpenChange,
  onParsed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onParsed: (parsed: ParsedConnectionUrl) => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Set when the form takes over: closing must not move focus back to the launcher.
  const handedOver = useRef(false);
  const id = useId();

  useEffect(() => {
    if (!open) return;
    setText("");
    setError(null);
    handedOver.current = false;
    // The drivers beyond the core ones read their own URLs: load them while the URL is pasted.
    void loadUrlSchemes();
  }, [open]);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    let parsed: ParsedConnectionUrl;
    try {
      parsed = parseConnectionUrl(text);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return;
    }
    handedOver.current = true;
    onOpenChange(false);
    onParsed(parsed);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-md"
        onCloseAutoFocus={(e) => {
          if (handedOver.current) e.preventDefault();
        }}
      >
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>New connection from URL</DialogTitle>
            <DialogDescription>
              Paste a postgres://, mysql://, mariadb:// or sqlite:// URL (over SSH:
              postgresql+ssh://sshuser@sshhost/user@host/db). You can review it before saving; the
              passwords go to the Keychain.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5">
            <Input
              aria-label="Connection URL"
              aria-invalid={error !== null}
              aria-describedby={error ? `${id}-error` : undefined}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setError(null);
              }}
              placeholder="postgresql://user:password@127.0.0.1:5432/database"
              className="font-mono"
              autoComplete="off"
              spellCheck={false}
              autoFocus
            />
            {error && (
              <p id={`${id}-error`} role="alert" className="text-error text-[11px]">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={text.trim() === ""}>
              Continue
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
