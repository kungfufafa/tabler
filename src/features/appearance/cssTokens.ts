/// <reference types="node" />
import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * Test helper: the custom properties `src/index.css` declares per theme, read from the file itself
 * (no CSS engine in jsdom), with `var()` references resolved the way the cascade does on <html>.
 */

type Decls = Record<string, string>;

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

/** The top-level rules of a stylesheet as selector → declarations (nested at-rules are skipped). */
export function topLevelRules(css: string): Map<string, Decls> {
  const text = stripComments(css);
  const out = new Map<string, Decls>();
  let depth = 0;
  let start = 0;
  let selector = "";
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === "{") {
      if (depth === 0) selector = text.slice(start, i).trim();
      depth++;
    } else if (ch === "}") {
      depth--;
      if (depth === 0) {
        const body = text.slice(text.indexOf("{", start) + 1, i);
        // Only flat rules: an at-rule with nested blocks holds braces in its body.
        if (!selector.startsWith("@") && !body.includes("{")) {
          const decls: Decls = out.get(selector) ?? {};
          for (const m of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) decls[m[1]!] = m[2]!.trim();
          out.set(selector, decls);
        }
        start = i + 1;
      }
    }
  }
  return out;
}

/** Replaces `var(--x)` (and `var(--x, fallback)`) with the declared values until none are left. */
export function resolveVars(value: string, decls: Decls, depth = 0): string {
  if (depth > 10) return value;
  return value.replace(/var\((--[\w-]+)(?:\s*,\s*([^)]+))?\)/g, (_, name: string, fb?: string) => {
    const found = decls[name];
    return found !== undefined ? resolveVars(found, decls, depth + 1) : (fb ?? "");
  });
}

export const INDEX_CSS = path.resolve(import.meta.dirname, "../../index.css");

export function readIndexCss(): string {
  return readFileSync(INDEX_CSS, "utf8");
}

export type ThemeCss = "light" | "dark" | "solarized-light" | "solarized-dark";

/** The declarations in effect on <html> for a built-in theme: `:root`, then `.dark`, then its own block. */
export function themeDecls(rules: Map<string, Decls>, theme: ThemeCss): Decls {
  const root = rules.get(":root") ?? {};
  const dark = rules.get(".dark") ?? {};
  const own = (sel: string) => rules.get(sel) ?? {};
  const named = `:root[data-theme-name="${theme}"]`;
  switch (theme) {
    case "light":
      return { ...root };
    case "dark":
      return { ...root, ...dark };
    case "solarized-light":
      return { ...root, ...own(named), ...own(`${named}:not([data-accent])`) };
    case "solarized-dark":
      return { ...root, ...dark, ...own(named), ...own(`${named}:not([data-accent])`) };
  }
}
