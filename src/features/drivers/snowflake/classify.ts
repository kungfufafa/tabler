import { classifyDialect } from "@/features/drivers/portable/classify";
import { asciiUpper, identName, scan } from "@/features/drivers/portable/scanner";
import type { StatementClass } from "@/features/safety/classify";
import type { DialectLex } from "@/lib/ipc/bindings/DialectLex";
import SNOWFLAKE_LEX from "@/lib/ipc/bindings/lex/snowflake.json";

/**
 * The backend's Snowflake classifier (`sqlgen/dialect/snowflake.rs`, `classify_snowflake`) for
 * Safe Mode and the mock: the portable classifier over the exported lexer profile and rules,
 * after the one check the rules cannot express (`quotes_nextval`).
 */

export const LEX = SNOWFLAKE_LEX as unknown as DialectLex;

/**
 * `snowflake::quotes_nextval`: a quoted name that is NEXTVAL in any ASCII case
 * (`seq."NEXTVAL"`), under any reading. The rules match bare words only, and whether Snowflake
 * advances a sequence for the quoted form is unproven, so it writes.
 */
export function quotesNextval(sql: string): boolean {
  return LEX.lex.readings.some((r) => {
    const s = scan(LEX.lex, r, sql);
    return s.tokens.some((t) => t.kind === "ident" && asciiUpper(identName(s, t)) === "NEXTVAL");
  });
}

/** `classify_snowflake`: a quoted NEXTVAL writes, everything else as the rules say. */
export function classifySnowflake(sql: string): StatementClass {
  if (quotesNextval(sql)) return "write";
  return classifyDialect(LEX, sql);
}
