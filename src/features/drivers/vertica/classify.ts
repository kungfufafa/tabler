import { classifyDialect } from "@/features/drivers/portable/classify";
import type { StatementClass } from "@/features/safety/classify";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import VERTICA_LEX from "@/lib/ipc/bindings/lex/vertica.json";

/**
 * The backend's Vertica classifier (`sqlgen/dialect/vertica.rs`, `classify_vertica`) for Safe Mode
 * and the mock: the portable classifier over the exported lexer profile, read on the raw text and
 * on its ASCII-folded spelling.
 */

export const LEX = VERTICA_LEX as unknown as DialectLex;

/** `vertica::fold_ascii_lookalikes`: each letter that case-folds to an ASCII letter, as that
 * letter upper-cased (the KELVIN SIGN folds to `k` and upper-cases to itself). */
export function foldAsciiLookalikes(sql: string): string {
  let changed = false;
  const out = [...sql].map((c) => {
    if (c.charCodeAt(0) < 0x80) return c;
    // Lower case finds the KELVIN SIGN; upper case LONG S (U+017F) and the dotless i.
    for (const cased of [c.toLowerCase(), c.toUpperCase()]) {
      const one = [...cased];
      if (one.length === 1 && /^[a-z]$/i.test(one[0]!)) {
        changed = true;
        return one[0]!.toUpperCase();
      }
    }
    return c;
  });
  return changed ? out.join("") : sql;
}

/**
 * `classify_vertica`: a write when the raw text or its folded spelling reads as one. The folded
 * text also folds strings and dollar tags, so it only adds writes; the raw text keeps the tags
 * byte-exact as the server does.
 */
export function classifyVertica(sql: string): StatementClass {
  if (classifyDialect(LEX, sql) === "write") return "write";
  const folded = foldAsciiLookalikes(sql);
  return folded === sql ? "read" : classifyDialect(LEX, folded);
}
