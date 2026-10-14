import type { BridgeMethod, PlainExecResult, PlainLoadedFile } from "./sandbox/protocol";
import { plainValue } from "./snapshot";
import type { HostServices } from "./types";

function text(method: BridgeMethod, args: readonly unknown[], i: number): string {
  const v = args[i];
  if (typeof v !== "string") throw new Error(`${method}: argument ${i + 1} must be text`);
  return v;
}

/**
 * Answers one call a sandboxed plugin makes (`context.execute`, `context.alert`,
 * `SystemService.insertToClipboard`…). The arguments come from untrusted code: each is checked
 * here, and the services apply the app's own rules (Safe Mode, http(s) links only…).
 */
export async function handleBridgeCall(
  services: HostServices,
  method: BridgeMethod,
  args: readonly unknown[],
): Promise<unknown> {
  switch (method) {
    case "execute": {
      const r = await services.execute(text(method, args, 0));
      const out: PlainExecResult = {
        columns: r.columns,
        rows: r.rows.map((row) => row.map(plainValue)),
        rowsAffected: r.rowsAffected,
      };
      return out;
    }
    case "alert":
      await services.alert(text(method, args, 0), text(method, args, 1));
      return null;
    case "insertToEditor":
      await services.insertToEditor(text(method, args, 0));
      return null;
    case "replaceSelection":
      await services.replaceSelection(text(method, args, 0));
      return null;
    case "insertToClipboard":
      await services.insertToClipboard(text(method, args, 0));
      return null;
    case "openURL":
      await services.openURL(text(method, args, 0));
      return null;
    case "notify":
      services.notify(text(method, args, 0), text(method, args, 1));
      return null;
    case "loadFile": {
      const file = await services.loadFile();
      const out: PlainLoadedFile = { name: file.name, text: file.text };
      return out;
    }
  }
}
