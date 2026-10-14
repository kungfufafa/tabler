import type { Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";
import type { BuiltinPlugin, PluginInvocation } from "../types";

const URL_IN_TEXT = /https?:\/\/[^\s"'<>]+/i;

function webUrl(text: string): string | null {
  try {
    const url = new URL(text);
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * The web link a cell holds: the whole value when it is an http(s) URL (`www.…` gets https://),
 * else the first http(s) URL inside the text. Null for anything else (mailto:, file:, plain text).
 */
export function urlFromValue(v: Value | null | undefined): string | null {
  if (!v) return null;
  const text = valueText(v)?.trim();
  if (!text) return null;
  const whole = /^www\.\S+\.\S+$/i.test(text) ? webUrl(`https://${text}`) : webUrl(text);
  if (whole) return whole;
  const found = URL_IN_TEXT.exec(text)?.[0]?.replace(/[.,;:!?)\]]+$/, "");
  return found ? webUrl(found) : null;
}

/** The clicked cell of `inv`. */
export function clickedValue(inv: PluginInvocation): Value | null {
  if (!inv.clickedRow || inv.clickedColumn === null) return null;
  return inv.clickedRow.cells[inv.clickedColumn] ?? null;
}

export const OPEN_URL: BuiltinPlugin = {
  identifier: "tabler.open-url",
  name: "Open URL",
  version: "1.0",
  author: "Tabler",
  description: "Opens the link in the clicked cell in your browser.",
  commands: [
    {
      name: "Open URL",
      contexts: ["grid"],
      run: async (inv, host) => {
        const url = urlFromValue(clickedValue(inv));
        if (!url) {
          host.notify(
            "Open URL",
            "The selected cell does not hold a web link (http:// or https://).",
          );
          return;
        }
        await host.openURL(url);
      },
    },
  ],
};
