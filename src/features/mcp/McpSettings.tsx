import { Copy, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { SettingGroup, SettingRow } from "@/features/settings/controls";
import {
  ipc,
  toIpcError,
  type McpClient,
  type McpClientStatus,
  type McpConfigPreview,
  type McpInfo,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { setPref, usePrefs, type Prefs } from "@/stores/prefs";
import { diffLines } from "./diff";

/** Clients registered through their CLI; the others through their mcp.json. */
const CLI_CLIENTS: readonly McpClient[] = ["claudeCode", "codex"];

async function copy(text: string, what: string): Promise<void> {
  try {
    await writeClipboard(text);
    toast.success(`Copied ${what}`);
  } catch {
    toast.error("Could not copy");
  }
}

function PrefSwitch({
  id,
  pref,
  disabled,
}: {
  id: string;
  pref: "mcpEnabled" | "mcpAllowWrites" | "mcpExposeProduction";
  disabled?: boolean;
}) {
  const value = usePrefs((p: Prefs) => p[pref]);
  return (
    <Switch id={id} checked={value} disabled={disabled} onCheckedChange={(v) => setPref(pref, v)} />
  );
}

/** The unified diff of a config file change. */
function ConfigDiff({ preview }: { preview: McpConfigPreview }) {
  const diff = diffLines(preview.before ?? "", preview.after);
  return (
    <pre
      aria-label="Changes"
      className="border-border bg-surface selectable max-h-72 overflow-auto rounded-md border py-1 font-mono text-[11px] leading-[1.45]"
    >
      {diff.map((line, i) =>
        line.kind === "skip" ? (
          <div key={i} className="text-muted-foreground px-2 italic">
            {`⋯ ${line.count} unchanged line${line.count === 1 ? "" : "s"}`}
          </div>
        ) : (
          <div
            key={i}
            data-kind={line.kind}
            className={cn(
              "px-2 whitespace-pre",
              line.kind === "add" && "bg-inserted",
              line.kind === "del" && "bg-deleted",
            )}
          >
            <span className="text-muted-foreground mr-2 select-none">
              {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "}
            </span>
            {line.text}
          </div>
        ),
      )}
    </pre>
  );
}

/**
 * Settings ▸ MCP Server (TablePlus b718 / b818): turns on `tabler --mcp`, its safety
 * switches (read-only and production connections hidden by default; TablePlus #3902), the
 * command line to register, and one-click install into Claude Code, Codex, Cursor and VS Code (and
 * VS Code Insiders).
 */
export function McpSettings() {
  const enabled = usePrefs((p) => p.mcpEnabled);
  // Loaded with each visit of the section (the CLIs may have been installed meanwhile).
  const [info, setInfo] = useState<{ data?: McpInfo; isError: boolean }>({ isError: false });
  const load = useCallback(() => {
    let live = true;
    ipc.mcp.info().then(
      (data) => live && setInfo({ data, isError: false }),
      () => live && setInfo({ isError: true }),
    );
    return () => {
      live = false;
    };
  }, []);
  useEffect(load, [load]);
  const [busy, setBusy] = useState<McpClient | null>(null);
  const [preview, setPreview] = useState<McpConfigPreview | null>(null);
  const [writing, setWriting] = useState(false);

  const labelOf = (client: McpClient) =>
    info.data?.clients.find((c) => c.client === client)?.label ?? client;

  const installCli = async (client: McpClient) => {
    const label = labelOf(client);
    const res = await ipc.mcp.installCli(client);
    if (res.ok) {
      toast.success(`Added Tabler to ${label}`, { description: res.message });
      return;
    }
    try {
      await writeClipboard(res.command);
    } catch {
      // The command is in the message below as well.
    }
    if (!res.cliFound)
      toast.message(`${label}'s command line tool was not found`, {
        description: `The command was copied; paste it in a terminal:\n${res.command}`,
        duration: 10_000,
      });
    else
      toast.error(`Could not add Tabler to ${label}`, {
        description: `${res.message}\nThe command was copied to run it yourself.`,
        duration: 10_000,
      });
  };

  const install = async (client: McpClient) => {
    setBusy(client);
    try {
      if (CLI_CLIENTS.includes(client)) {
        await installCli(client);
        return;
      }
      const p = await ipc.mcp.previewConfig(client);
      if (p.changed) setPreview(p);
      else toast.success(`Tabler is already set up in ${labelOf(client)}`, { description: p.path });
    } catch (e) {
      toast.error(`Could not set up ${labelOf(client)}`, {
        description: toIpcError(e).message,
      });
    } finally {
      setBusy(null);
    }
  };

  const write = async (p: McpConfigPreview) => {
    setWriting(true);
    try {
      const path = await ipc.mcp.writeConfig(p.client, p.before);
      const label = labelOf(p.client);
      toast.success(`Added Tabler to ${label}`, {
        description: `${path}\nRestart ${label} or reload its MCP servers to use it.`,
      });
      setPreview(null);
      load();
    } catch (e) {
      toast.error("Could not save the file", { description: toIpcError(e).message });
    } finally {
      setWriting(false);
    }
  };

  const data = info.data;
  const clientTitle = (c: McpClientStatus) => {
    if (CLI_CLIENTS.includes(c.client))
      return c.detected
        ? `Runs ${c.detail ?? "the CLI"} mcp add`
        : "The command line tool was not found: copies the command instead";
    return c.detail ? `Adds Tabler to ${c.detail}` : undefined;
  };

  return (
    <>
      <SettingGroup title="MCP Server">
        <SettingRow
          id="pref-mcp-enabled"
          label="Enable MCP server"
          description="AI tools such as Claude Code, Codex, Cursor or VS Code can list your connections, read their structure and run queries through Tabler."
        >
          <PrefSwitch id="pref-mcp-enabled" pref="mcpEnabled" />
        </SettingRow>
        <SettingRow
          id="pref-mcp-allow-writes"
          label="Allow MCP clients to change data"
          description="Only on connections in Silent Mode that are not read-only. When off, MCP queries can only read."
        >
          <PrefSwitch id="pref-mcp-allow-writes" pref="mcpAllowWrites" disabled={!enabled} />
        </SettingRow>
        <SettingRow
          id="pref-mcp-expose-production"
          label="Expose production connections"
          description="Connections tagged Production stay hidden from MCP clients unless this is on."
        >
          <PrefSwitch
            id="pref-mcp-expose-production"
            pref="mcpExposeProduction"
            disabled={!enabled}
          />
        </SettingRow>
        <SettingRow
          label="Command"
          description={
            <>
              <span
                className="selectable text-foreground/90 block font-mono text-[11px] break-all"
                title={data?.commandLine}
              >
                {data?.commandLine ?? (info.isError ? "Unavailable" : "…")}
              </span>
              Register this stdio server in any MCP client. Every MCP query is logged in the
              History.
              {data?.warning && (
                <span className="text-warning mt-1 flex items-start gap-1">
                  <TriangleAlert className="mt-0.5 size-3 shrink-0" />
                  {data.warning}
                </span>
              )}
            </>
          }
        >
          <Button
            size="xs"
            variant="outline"
            disabled={!data}
            onClick={() => data && void copy(data.commandLine, "the command")}
          >
            <Copy />
            Copy
          </Button>
        </SettingRow>
        <div className="flex flex-col gap-2 px-3 py-2.5">
          <div className="flex flex-col gap-0.5">
            <span id="mcp-install-label" className="text-[13px] leading-5">
              Install in
            </span>
            <span className="text-muted-foreground text-[11px] leading-4">
              {enabled
                ? "Adds Tabler to the app's MCP servers. Cursor and VS Code (Insiders) show the change to their mcp.json first."
                : "Turn on the MCP server first."}
            </span>
          </div>
          <div role="group" aria-labelledby="mcp-install-label" className="flex flex-wrap gap-1.5">
            {(data?.clients ?? []).map((c) => (
              <Button
                key={c.client}
                size="xs"
                variant="outline"
                disabled={!enabled || busy !== null}
                title={clientTitle(c)}
                onClick={() => void install(c.client)}
              >
                {c.label}
                {!c.detected && CLI_CLIENTS.includes(c.client) && (
                  <span className="text-muted-foreground">(copy)</span>
                )}
              </Button>
            ))}
          </div>
        </div>
      </SettingGroup>
      <AlertDialog open={preview !== null} onOpenChange={(o) => !o && !writing && setPreview(null)}>
        <AlertDialogContent className="data-[size=default]:sm:max-w-xl">
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">
              {preview && `Add Tabler to ${labelOf(preview.client)}?`}
            </AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              {preview?.before === null ? "Tabler creates " : "Tabler changes "}
              <span className="font-mono text-[11px] break-all">{preview?.path}</span>
              {preview?.before === null
                ? "."
                : ". Other servers and settings in the file stay as they are."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {preview && <ConfigDiff preview={preview} />}
          <AlertDialogFooter>
            <AlertDialogCancel size="sm" disabled={writing}>
              Cancel
            </AlertDialogCancel>
            <Button size="sm" disabled={writing} onClick={() => preview && void write(preview)}>
              {preview?.before === null ? "Create File" : "Save Changes"}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
