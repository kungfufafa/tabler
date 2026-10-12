import { RotateCw, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { DumpBinary, DumpTool, ParsedVersion } from "@/lib/ipc";
import { binaryLabel, versionWarning } from "./options";
import { Field } from "./parts";

const folder = (path: string) => path.slice(0, path.lastIndexOf("/")) || "/";

/**
 * "Choose version": every binary of `tool` found on this Mac, with the warning when the chosen
 * one is older than the server.
 */
export function VersionPicker({
  tool,
  binaries,
  selected,
  server,
  loading,
  onSelect,
  onRescan,
  disabled,
}: {
  tool: DumpTool;
  binaries: readonly DumpBinary[];
  selected: DumpBinary | null;
  server: ParsedVersion | null;
  loading: boolean;
  onSelect: (path: string) => void;
  onRescan: () => void;
  disabled?: boolean;
}) {
  const list = binaries.filter((b) => b.tool === tool);
  const warning = versionWarning(selected, server);
  const id = `backup-version-${tool}`;
  return (
    <Field
      label="Choose version"
      htmlFor={id}
      hint={
        list.length === 0 && !loading ? (
          <span className="text-warning">
            No {tool} found in PATH, Homebrew, Postgres.app or /usr/local. Install it (e.g.{" "}
            <span className="font-mono">
              brew install {tool.startsWith("pg") || tool === "psql" ? "libpq" : "mysql-client"}
            </span>
            ) and rescan.
          </span>
        ) : selected ? (
          <span className="font-mono break-all" title={selected.path}>
            {selected.path}
          </span>
        ) : null
      }
    >
      <div className="flex items-center gap-1">
        <Select
          value={selected?.path ?? ""}
          onValueChange={onSelect}
          disabled={disabled || list.length === 0}
        >
          <SelectTrigger
            id={id}
            size="sm"
            className="min-w-0 flex-1 text-xs"
            aria-label="Choose version"
          >
            <SelectValue placeholder={loading ? "Looking for binaries…" : `No ${tool} found`}>
              {selected ? `${binaryLabel(selected)} — ${tool}` : undefined}
            </SelectValue>
          </SelectTrigger>
          <SelectContent>
            {list.map((b) => (
              <SelectItem key={b.path} value={b.path} className="text-xs">
                <span className="flex min-w-0 flex-col">
                  <span>
                    {binaryLabel(b)} <span className="text-muted-foreground">— {tool}</span>
                  </span>
                  <span className="text-muted-foreground max-w-80 truncate font-mono text-[10px]">
                    {folder(b.path)}
                  </span>
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Rescan for binaries"
          title="Rescan for binaries"
          onClick={onRescan}
          disabled={disabled || loading}
        >
          <RotateCw className={loading ? "animate-spin" : undefined} />
        </Button>
      </div>
      {warning && (
        <div role="note" className="text-warning flex items-start gap-1.5 text-[11px] leading-4">
          <TriangleAlert className="mt-px size-3.5 shrink-0" />
          <span>{warning}</span>
        </div>
      )}
    </Field>
  );
}
