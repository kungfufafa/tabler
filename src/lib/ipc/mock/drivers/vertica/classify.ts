import {
  callsListedFunction,
  classifyDialect,
  isSingleStatement,
} from "@/features/drivers/portable/classify";
import {
  identName,
  isPunct,
  PLAIN,
  scan,
  upper,
  type Token,
} from "@/features/drivers/portable/scanner";
import type { StatementClass } from "@/features/safety/classify";
import type { DialectLex } from "../../../bindings/DialectLex";
import VERTICA_LEX from "../../../bindings/lex/vertica.json";

/**
 * The backend's Vertica classifier and the session's refusals (`sqlgen/dialect/vertica.rs`,
 * `drivers/vertica/session.rs`), for the mock: the portable classifier over the exported lexer
 * profile, read on the raw text and on its ASCII-folded spelling (`classify_vertica`), the
 * `s.NEXTVAL` form of a sequence's next value (`advances_sequence`), and what a read-only
 * session, a grid read and every statement refuse.
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

/** `classify_vertica`: a write when the raw text or its folded spelling reads as one. */
export function classifyVertica(sql: string): StatementClass {
  if (classifyDialect(LEX, sql) === "write") return "write";
  const folded = foldAsciiLookalikes(sql);
  return folded === sql ? classifyDialect(LEX, sql) : classifyDialect(LEX, folded);
}

/** `sql::writes` on the Vertica dialect. */
export const writes = (sql: string): boolean => classifyVertica(sql) === "write";

/** `vertica::names_next_value` under every reading. */
function namesNextValue(sql: string): boolean {
  const readings = LEX.lex.readings.length > 0 ? LEX.lex.readings : [PLAIN];
  return readings.some((r) => {
    const s = scan(LEX.lex, r, sql);
    const code: Token[] = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
    const name = (t: Token): string | null =>
      t.kind === "word" ? upper(s, t) : t.kind === "ident" ? asciiUpperOf(identName(s, t)) : null;
    return (
      s.ambiguous ||
      code.some((t, i) => {
        const next = code[i + 1];
        return next !== undefined && isPunct(s, t, ".") && name(next) === "NEXTVAL";
      }) ||
      code.some((t, i) => {
        const [u, amp, ident] = [code[i + 1], code[i + 2], code[i + 3]];
        return (
          isPunct(s, t, ".") &&
          u?.kind === "word" &&
          upper(s, u) === "U" &&
          amp !== undefined &&
          isPunct(s, amp, "&") &&
          ident?.kind === "ident"
        );
      })
    );
  });
}

/** Rust's `to_ascii_uppercase` of a quoted name: only ASCII letters change. */
const asciiUpperOf = (s: string): string => s.replace(/[a-z]/g, (c) => c.toUpperCase());

/** `vertica::advances_sequence`: `s.NEXTVAL` in the raw or the folded text. */
export function advancesSequence(sql: string): boolean {
  const folded = foldAsciiLookalikes(sql);
  return namesNextValue(sql) || (folded !== sql && namesNextValue(folded));
}

/** `SPEC.knobs.side_effect_functions`. */
export const SIDE_EFFECT_FUNCTIONS = ["NEXTVAL", "SLEEP"] as const;

/** `session::refused_read_only`. */
export const refusedReadOnly = (sql: string): boolean =>
  writes(sql) || callsListedFunction(LEX.lex, SIDE_EFFECT_FUNCTIONS, sql) || advancesSequence(sql);

/** `sql::is_single_statement` on the Vertica dialect. */
export const isSingle = (sql: string): boolean => isSingleStatement(LEX.lex, LEX.classify, sql);
