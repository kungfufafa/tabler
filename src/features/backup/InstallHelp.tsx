import { Download, RotateCw, Terminal, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { toIpcError } from "@/lib/ipc";
import { installWithBrew, openDownloadPage, type InstallHelp } from "./install";

/**
 * Under "Choose version" when no usable binary is installed: "Install with Homebrew…" (the
 * command runs in Terminal, where the user confirms it), the download page, and Rescan.
 */
export function InstallHelpNote({
  help,
  loading,
  onRescan,
  disabled,
}: {
  help: InstallHelp;
  loading: boolean;
  onRescan: () => void;
  disabled?: boolean;
}) {
  const [started, setStarted] = useState<string | null>(null);
  const install = async (formula: string) => {
    try {
      await installWithBrew(formula);
      setStarted(formula);
    } catch (e) {
      toast.error("Could not open Terminal", { description: toIpcError(e).message });
    }
  };
  const download = () =>
    void openDownloadPage(help.download.url).catch((e: unknown) =>
      toast.error("Could not open the browser", { description: toIpcError(e).message }),
    );
  const [main, ...others] = help.formulas;
  return (
    <div
      role="note"
      className="border-warning/40 bg-warning/5 flex flex-col gap-1.5 rounded-md border px-2 py-1.5 text-[11px] leading-4"
    >
      <div className="text-warning flex items-start gap-1.5">
        <TriangleAlert className="mt-px size-3.5 shrink-0" />
        <span>{help.reason}</span>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {main && (
          <Button
            type="button"
            variant="outline"
            size="xs"
            disabled={disabled}
            title={`Runs “brew install ${main.formula}” in Terminal`}
            onClick={() => void install(main.formula)}
          >
            <Terminal /> Install with Homebrew…
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={disabled || loading}
          onClick={onRescan}
        >
          <RotateCw className={loading ? "animate-spin" : undefined} /> Rescan
        </Button>
        <Button
          type="button"
          variant="link"
          size="xs"
          className="h-auto px-0 text-[11px]"
          onClick={download}
        >
          <Download /> {help.download.label}
        </Button>
      </div>
      {main && (
        <div className="text-muted-foreground">
          Installs <span className="font-mono">{main.formula}</span>
          {others.length > 0 && (
            <>
              {" "}
              (or{" "}
              {others.map((o, i) => (
                <span key={o.formula}>
                  {i > 0 && ", "}
                  <button
                    type="button"
                    disabled={disabled}
                    title={o.label}
                    onClick={() => void install(o.formula)}
                    className="text-foreground font-mono underline-offset-2 hover:underline"
                  >
                    {o.formula}
                  </button>
                </span>
              ))}
              )
            </>
          )}
          .
        </div>
      )}
      {started && (
        <div role="status" className="text-foreground">
          Terminal opened to install <span className="font-mono">{started}</span>: press Return
          there, then Rescan here when it is done.
        </div>
      )}
    </div>
  );
}
