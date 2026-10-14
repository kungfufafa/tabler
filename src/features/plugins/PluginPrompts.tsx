import {
  ClipboardCopy,
  FileText,
  Globe,
  PencilLine,
  ShieldAlert,
  SquareTerminal,
  TableProperties,
} from "lucide-react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { answerPluginPrompt, usePluginsStore, type PluginPrompt } from "./store";

/** What a third-party plugin can do once enabled (the permission prompt lists it). */
const PLUGIN_CAPABILITIES = [
  {
    icon: TableProperties,
    text: "Read the clicked item, the selected rows and the SQL editor's text when you run it",
  },
  {
    icon: SquareTerminal,
    text: "Run SQL on the connection (Safe Mode still asks; read-only connections refuse changes)",
  },
  { icon: PencilLine, text: "Insert text into the SQL editor" },
  { icon: ClipboardCopy, text: "Write to the clipboard" },
  { icon: FileText, text: "Read a file you pick (in a file dialog; text files up to 5 MB)" },
  {
    icon: Globe,
    text: "Open web links in your browser (it asks the first time). A link's address can carry data, such as rows or SQL, to any website",
  },
] as const;

function Body({ prompt }: { prompt: PluginPrompt }) {
  switch (prompt.kind) {
    case "alert":
      return (
        <AlertDialogHeader>
          <AlertDialogTitle>{prompt.title || prompt.plugin}</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2">
              {prompt.message && (
                <p className="max-h-[50vh] overflow-y-auto text-xs break-words whitespace-pre-wrap">
                  {prompt.message}
                </p>
              )}
              <p className="text-muted-foreground text-[11px]">From the plugin “{prompt.plugin}”</p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
      );
    case "permission":
      return (
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            <ShieldAlert className="text-warning size-4" /> Enable “{prompt.plugin.name}”?
          </AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2 text-xs">
              <p>
                {prompt.plugin.author ? `By ${prompt.plugin.author}. ` : ""}Third-party plugins are
                not reviewed by Tabler. Its scripts run in a sandbox without network access; while
                it is enabled, it can:
              </p>
              <ul className="flex flex-col gap-1">
                {PLUGIN_CAPABILITIES.map(({ icon: Icon, text }) => (
                  <li key={text} className="flex items-start gap-2">
                    <Icon className="mt-px size-3.5 shrink-0" aria-hidden />
                    <span>{text}</span>
                  </li>
                ))}
              </ul>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
      );
    case "openUrl":
      return (
        <AlertDialogHeader>
          <AlertDialogTitle>Open this link?</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className="flex flex-col gap-2 text-xs">
              <p>The plugin “{prompt.plugin}” wants to open:</p>
              <p className="bg-muted text-foreground rounded px-2 py-1 font-mono text-[11px] break-all">
                {prompt.url}
              </p>
              <p className="text-muted-foreground text-[11px]">
                A link's address can carry data from your database. Tabler will not ask again for
                this plugin.
              </p>
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
      );
  }
}

const CONFIRM: Record<PluginPrompt["kind"], string> = {
  alert: "OK",
  permission: "Enable",
  openUrl: "Open Link",
};

/** The plugins' dialogs, one at a time: `context.alert`, permission prompts, opening links. */
export function PluginPrompts() {
  const prompt = usePluginsStore((s) => s.prompts[0]?.prompt ?? null);
  if (!prompt) return null;
  return (
    <AlertDialog
      key={prompt.id}
      open
      onOpenChange={(o) => !o && answerPluginPrompt(prompt.id, prompt.kind === "alert")}
    >
      <AlertDialogContent>
        <Body prompt={prompt} />
        <AlertDialogFooter>
          {prompt.kind !== "alert" && (
            <AlertDialogCancel onClick={() => answerPluginPrompt(prompt.id, false)}>
              Cancel
            </AlertDialogCancel>
          )}
          <AlertDialogAction onClick={() => answerPluginPrompt(prompt.id, true)}>
            {CONFIRM[prompt.kind]}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
