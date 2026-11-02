import { callsListedFunction, isSingleStatement } from "@/features/drivers/portable/classify";
import {
  identName,
  isPunct,
  PLAIN,
  scan,
  upper,
  type Token,
} from "@/features/drivers/portable/scanner";
import { classifyVertica, foldAsciiLookalikes, LEX } from "@/features/drivers/vertica/classify";

/**
 * The session's refusals (`drivers/vertica/session.rs`) for the mock, over the renderer's copy
 * of the backend's classifier (`classify_vertica`, which Safe Mode reads too): the `s.NEXTVAL`
 * form of a sequence's next value (`advances_sequence`), and what a read-only session, a grid
 * read and every statement refuse.
 */

export { classifyVertica, foldAsciiLookalikes, LEX };

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
