import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { toast } from "sonner";
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
import { Label } from "@/components/ui/label";
import { useAction } from "@/features/palette/actions";
import { ipc, toIpcError } from "@/lib/ipc";
import { useActiveConnection, useConnectionsStore } from "@/stores/connections";
import {
  applyEncoding,
  encodingKey,
  isUtf8Encoding,
  NON_UTF8_NOTE,
  openEncodingDialog,
  useEncodingDialog,
} from "./encoding";
import { ENCODING_CHOICES } from "./schema";
import { StartupConnectionHost } from "./StartupConnection";
import { useProfiles } from "./useConnections";

/**
 * Connection ▸ View Using Encoding… (§11.7, b170): reads the open connection's data with another
 * encoding (MySQL `SET NAMES`, PostgreSQL `client_encoding`), e.g. latin1 to fix text that shows
 * as mojibake. The backend reopens the session with it; the open tabs reload. The saved
 * connection keeps its own encoding (Advanced ▸ Encoding), which a reconnect goes back to.
 * Tabler reads and writes text as UTF-8 only: with another encoding the session is read-only (the
 * backend refuses writes and the connection's `readOnly` guard blocks them up front), which the
 * dialog says before and after applying it.
 */

function EncodingDialog() {
  const uid = useId();
  const qc = useQueryClient();
  const connectionId = useEncodingDialog((s) => s.connectionId);
  const conn = useConnectionsStore((s) => (connectionId ? s.byId[connectionId] : undefined));
  const profiles = useProfiles();
  const driver = conn?.info.driver;
  const server = driver === "mysql" || driver === "postgres" ? driver : null;
  const current = useQuery({
    queryKey: encodingKey(connectionId ?? ""),
    queryFn: () => ipc.connections.getEncoding(connectionId!),
    enabled: connectionId !== null && server !== null,
    staleTime: 0,
  });
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (current.data) setValue(current.data);
  }, [current.data]);
  const profile = profiles.data?.find((p) => p.id === conn?.info.profileId);
  const own = profile?.options.encoding || (server === "mysql" ? "utf8mb4" : "UTF8");
  const choices = useMemo(() => (server ? ENCODING_CHOICES[server] : []), [server]);
  const close = () => {
    useEncodingDialog.setState({ connectionId: null });
    setBusy(false);
  };
  const valid = /^[A-Za-z0-9_-]{1,40}$/.test(value.trim());
  const apply = async (encoding: string | null) => {
    if (!connectionId) return;
    setBusy(true);
    const ok = await applyEncoding(qc, connectionId, encoding);
    void qc.invalidateQueries({ queryKey: encodingKey(connectionId) });
    setBusy(false);
    if (ok) {
      const reads = encoding ?? own;
      const name = conn?.info.name ?? "The connection";
      toast.success(
        isUtf8Encoding(reads)
          ? `${name} now reads ${reads}`
          : `${name} now reads ${reads} and is read-only`,
      );
      close();
    }
  };

  return (
    <Dialog open={connectionId !== null} onOpenChange={(o) => !o && close()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>View Using Encoding</DialogTitle>
          <DialogDescription>
            {server
              ? `Reads the data of ${conn?.info.name ?? "the connection"} with another encoding; the open tabs reload. Reconnecting goes back to ${own}.`
              : "SQLite text is always UTF-8: there is no connection encoding."}
          </DialogDescription>
        </DialogHeader>
        {server && (
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (valid && !busy) void apply(value.trim());
            }}
          >
            <div className="flex items-center gap-3">
              <Label htmlFor={`${uid}-enc`} className="text-muted-foreground text-xs font-normal">
                Encoding
              </Label>
              <Input
                id={`${uid}-enc`}
                value={value}
                onChange={(e) => setValue(e.target.value)}
                list={`${uid}-choices`}
                className="w-56 font-mono"
                autoComplete="off"
                autoFocus
                aria-invalid={value !== "" && !valid ? true : undefined}
              />
              <datalist id={`${uid}-choices`}>
                {choices.map((c) => (
                  <option key={c} value={c} />
                ))}
              </datalist>
              {current.isFetching && <Loader2 className="size-3.5 animate-spin" />}
            </div>
            <p className="text-muted-foreground text-[11px]">
              Now: <span className="font-mono">{current.data ?? "…"}</span>
              {current.isError ? ` (${toIpcError(current.error).message})` : ""}
            </p>
            {valid && !isUtf8Encoding(value) && (
              <p role="note" className="text-warning text-[11px]">
                {NON_UTF8_NOTE}
              </p>
            )}
            <DialogFooter>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => void apply(null)}
                title="Use the connection's own encoding"
              >
                Reset to {own}
              </Button>
              <Button type="button" variant="ghost" size="sm" onClick={close}>
                Cancel
              </Button>
              <Button type="submit" size="sm" disabled={!valid || busy}>
                {busy && <Loader2 className="animate-spin" />}
                Apply
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Registers Connection ▸ View Using Encoding… while a MySQL / PostgreSQL connection is active. */
function useEncodingAction() {
  const active = useActiveConnection();
  const id = active?.info.connectionId ?? null;
  const server = active?.info.driver === "mysql" || active?.info.driver === "postgres";
  const open = useCallback(() => {
    if (id) openEncodingDialog(id);
  }, [id]);
  useAction("connection.encoding", id && server ? open : null);
}

/**
 * Connection extras mounted once by the app shell: View Using Encoding… and the "At launch"
 * default connection.
 */
export function ConnectionExtrasHost() {
  useEncodingAction();
  return (
    <>
      <EncodingDialog />
      <StartupConnectionHost />
    </>
  );
}
